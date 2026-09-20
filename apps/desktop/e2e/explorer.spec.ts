// The Explorer flow against the mocked backend (`vite --mode mock`): the simulated scan
// keeps its browser pace (150 ms per tick) so the progress panel is visible.

// `test` comes from `./fixtures`, not from Playwright: it runs every one of these under the
// window's own Content Security Policy and fails the test that trips it.
import { expect, test, type Locator, type Page } from './fixtures';

const ROOT = '/Users/demo';
const PERMISSION_DENIED = 'Operation not permitted (os error 1)';

interface MockWindow {
  __STORAGE_MONITOR_MOCK__?: { revealed: string[] };
}

function crumbs(page: Page): Locator {
  return page.getByRole('navigation', { name: 'Breadcrumb' }).getByRole('listitem');
}

// The name cell holds the name in a span titled with it; the lock/info markers next to
// it are spans too, told apart by `data-marker`. It is the second cell, not the first: the
// Explorer hands the table a selection, which puts a column of tick boxes in front.
const NAME_SPAN = 'td:nth-child(2) span[title]:not([data-marker])';

/** The names in the table, in display order. */
function names(page: Page): Locator {
  return page.getByTestId('node-rows').locator(NAME_SPAN);
}

function row(page: Page, name: string): Locator {
  return page
    .getByTestId('node-rows')
    .locator('tr')
    .filter({ has: page.locator(`${NAME_SPAN}[title="${name}"]`) });
}

/** The Δ cell: tick box, name, size, %, Δ. */
function delta(page: Page, name: string): Locator {
  return row(page, name).getByRole('cell').nth(4);
}

/** Opens the app, starts a scan and waits for the table of the root. */
async function scan(page: Page): Promise<void> {
  await page.goto('/');
  await page.getByRole('button', { name: 'Scan', exact: true }).click();
  await expect(page.getByRole('table')).toBeVisible();
}

test('scans the home folder: empty state, progress, summary, table and treemap', async ({
  page,
}) => {
  await page.goto('/');
  await expect(page.getByText(ROOT, { exact: true })).toBeVisible();
  await expect(page.getByText(/Scans the home folder/)).toBeVisible();
  await expect(page.getByRole('table')).toBeHidden();

  await page.getByRole('button', { name: 'Scan', exact: true }).click();
  const progress = page.getByRole('progressbar', { name: 'Scanning' });
  await expect(progress).toBeVisible();
  await expect(page.getByRole('button', { name: 'Cancel', exact: true })).toBeVisible();
  const currentPath = page.getByTestId('scan-current-path');
  const firstPath = await currentPath.textContent();
  expect(firstPath).toContain('/');
  await expect(currentPath).not.toHaveText(firstPath ?? '');

  await expect(page.getByRole('table')).toBeVisible();
  await expect(progress).toBeHidden();
  await expect(page.getByRole('heading', { name: 'demo', exact: true })).toBeVisible();
  await expect(page.getByTestId('scan-summary')).toHaveText(
    /^[\d.]+ GB · \d+ files · \d+ folders · scanned in [\d.]+ s · \d+ read errors$/,
  );
  await expect(page.getByRole('meter', { name: 'Disk usage' })).toBeVisible();
  await expect(page.getByRole('button', { name: 'Rescan', exact: true })).toBeVisible();
  await expect(crumbs(page)).toHaveText(['demo']);
  await expect(names(page).first()).toHaveText('Library');
  await expect(names(page)).toHaveCount(9);
  await expect(page.getByTestId('treemap').locator('canvas')).toBeVisible();
});

test('drills down by click and by keyboard, and comes back', async ({ page }) => {
  await scan(page);

  await row(page, 'Library').click();
  await expect(crumbs(page)).toHaveText(['demo', 'Library']);
  await expect(names(page)).toHaveText([
    'Developer',
    'Containers',
    'Caches',
    'Application Support',
  ]);

  await page
    .getByRole('navigation', { name: 'Breadcrumb' })
    .getByRole('button', { name: 'demo', exact: true })
    .click();
  await expect(crumbs(page)).toHaveText(['demo']);
  await expect(names(page).first()).toHaveText('Library');

  await row(page, 'Library').focus();
  await page.keyboard.press('Enter');
  await expect(crumbs(page)).toHaveText(['demo', 'Library']);
  // The first row of the new directory takes the focus, so Enter keeps going down.
  await expect(row(page, 'Developer')).toBeFocused();
  await page.keyboard.press('Enter');
  await expect(crumbs(page)).toHaveText(['demo', 'Library', 'Developer']);

  await page.keyboard.press('Backspace');
  await expect(crumbs(page)).toHaveText(['demo', 'Library']);
  await page.keyboard.press('Backspace');
  await expect(crumbs(page)).toHaveText(['demo']);
});

test('shows growth and shrinkage since the previous snapshot', async ({ page }) => {
  await scan(page);
  await expect(delta(page, 'Library')).toHaveText('+6.4 GB');
  await expect(delta(page, 'Downloads')).toHaveText('−4.2 GB');
  await expect(delta(page, 'Pictures')).toBeEmpty();
});

test('marks an unreadable directory with a lock and a skipped volume with an info sign', async ({
  page,
}) => {
  await scan(page);
  const lock = row(page, '.Trash').getByRole('img', { name: PERMISSION_DENIED });
  await expect(lock).toHaveAttribute('data-marker', 'lock');
  await expect(lock).toHaveAttribute('title', PERMISSION_DENIED);

  const info = row(page, 'OrbStack').getByRole('img', { name: 'Not scanned: different volume' });
  await expect(info).toHaveAttribute('data-marker', 'info');
  await expect(row(page, 'Library').getByRole('img')).toHaveCount(0);
});

test('reveals a row in Finder from the button that shows fully on hover', async ({ page }) => {
  await scan(page);
  const downloads = row(page, 'Downloads');
  const reveal = downloads.getByRole('button', { name: 'Reveal in Finder' });
  // Visible at rest as an affordance, fully opaque on the hovered row.
  await expect(reveal).toHaveCSS('opacity', '0.35');
  await downloads.hover();
  await expect(reveal).toHaveCSS('opacity', '1');

  await reveal.click();
  await expect
    .poll(() => page.evaluate(() => (window as MockWindow).__STORAGE_MONITOR_MOCK__?.revealed))
    .toEqual([`${ROOT}/Downloads`]);
  await expect(crumbs(page)).toHaveText(['demo']);
});

test('cancelling a scan keeps the partial results and says so', async ({ page }) => {
  await page.goto('/');
  await page.getByRole('button', { name: 'Scan', exact: true }).click();
  await page.getByRole('button', { name: 'Cancel', exact: true }).click();
  await expect(page.getByText('Scan cancelled: partial results.')).toBeVisible();
  await expect(page.getByRole('table')).toBeVisible();
  await expect(names(page).first()).toHaveText('Library');
});

test.describe('screenshots for the PR', () => {
  // Tall enough for the whole table: the content area scrolls on its own.
  test.use({ viewport: { width: 1280, height: 900 } });

  test('explorer.png (light) and explorer-dark.png (dark)', async ({ page }) => {
    await page.emulateMedia({ colorScheme: 'light' });
    await scan(page);
    await expect(page.getByTestId('treemap').locator('canvas')).toBeVisible();
    await expect(page.getByRole('meter', { name: 'Disk usage' })).toBeVisible();
    await page.screenshot({ path: test.info().outputPath('explorer.png'), fullPage: true });

    await page.emulateMedia({ colorScheme: 'dark' });
    await page.screenshot({ path: test.info().outputPath('explorer-dark.png'), fullPage: true });
  });
});
