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

/** The tick box of a row, by the label the table gives it. */
function box(page: Page, name: string): Locator {
  return row(page, name).getByRole('checkbox');
}

/**
 * The action bar. It is in the layout whether or not anything is ticked, so it is always
 * here; `aria-hidden` is how it says whether it has anything to say.
 */
function bar(page: Page): Locator {
  return page.getByTestId('selection-bar');
}

/** Opens the app, starts a scan and waits for the table of the root. */
async function scan(page: Page): Promise<void> {
  await page.goto('/');
  await page.getByRole('button', { name: 'Scan', exact: true }).click();
  await expect(page.getByRole('table')).toBeVisible();
}

/**
 * The size out of a line the app printed, spelled the way it printed it: "2 items · 50.8
 * GB" → "50.8 GB". As the string, because that is what the next assertion compares against
 * — `Number("51.0")` prints itself back as "51", which no line in this app says.
 */
async function printedSize(text: Locator): Promise<string> {
  const printed = (await text.textContent()) ?? '';
  const size = /[\d.]+ GB/.exec(printed);
  if (size === null) {
    throw new Error(`"${printed}" carries no size in GB`);
  }
  return size[0];
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

test('moves two ticked rows to the Trash, and shrinks the header by what it promised', async ({
  page,
}) => {
  await scan(page);
  const before = Number.parseFloat(await printedSize(page.getByTestId('scan-summary')));
  // Ticked at the root, deliberately. A batch that patched anything sends the Explorer back
  // to `ROOT_ID`, so a batch run from inside a directory leaves that directory whatever
  // happened — and "the rows are gone" would then be the page having moved, not a deletion.
  await expect(crumbs(page)).toHaveText(['demo']);
  await expect(bar(page)).toHaveAttribute('aria-hidden', 'true');

  await box(page, 'Downloads').check();
  await box(page, 'Movies').check();
  await expect(bar(page)).not.toHaveAttribute('aria-hidden', 'true');
  // The live region, which is where a ticked row is announced at all.
  await expect(page.getByTestId('selection-status')).toHaveText(/^2 items selected · [\d.]+ GB$/);

  await bar(page).getByRole('button', { name: 'Move to Trash' }).click();
  const dialog = page.getByRole('dialog');
  await expect(dialog.getByRole('heading')).toHaveText('Move 2 items to the Trash?');
  await expect(dialog.getByTestId('delete-entries').locator('span[title]')).toHaveText([
    `${ROOT}/Downloads`,
    `${ROOT}/Movies`,
  ]);
  const total = dialog.getByTestId('delete-total');
  await expect(total).toHaveText(/^2 items · [\d.]+ GB$/);
  const promised = await printedSize(total);

  // The dialog's button, not the bar's: one word apart on purpose.
  await dialog.getByRole('button', { name: 'Move to the Trash' }).click();
  // What the report says it freed is, to the character, what the dialog promised.
  await expect(dialog.getByTestId('result-summary')).toHaveText(
    `Moved 2 items to the Trash · ${promised}`,
  );
  await dialog.getByRole('button', { name: 'Close' }).click();
  await expect(dialog).toBeHidden();

  // Both rows gone, and the ones around them still here — a table that emptied itself would
  // satisfy "the rows are gone" just as well.
  await expect(names(page)).toHaveText([
    'Library',
    'Pictures',
    'src',
    'Documents',
    '.zshrc',
    '.Trash',
    'OrbStack',
  ]);
  await expect(crumbs(page)).toHaveText(['demo']);
  await expect(bar(page)).toHaveAttribute('aria-hidden', 'true');
  // A batch emits no event, so the header is only right if the page asked for it again.
  // Within half a unit of what the dialog promised: both numbers are rounded to one
  // decimal before they are printed, and it is the printed ones that are being compared.
  await expect
    .poll(async () => Number.parseFloat(await printedSize(page.getByTestId('scan-summary'))))
    .toBeCloseTo(before - Number.parseFloat(promised), 0);
});

test('deletes what it can when the batch holds a folder this app never deletes from', async ({
  page,
}) => {
  await scan(page);
  await box(page, 'Library').check();
  await box(page, 'Movies').check();
  await bar(page).getByRole('button', { name: 'Move to Trash' }).click();

  const dialog = page.getByRole('dialog');
  const total = dialog.getByTestId('delete-total');
  await expect(total).toHaveText(/· 1 blocked$/);
  await expect(dialog.getByTestId('block-reason')).toHaveText(
    'Inside a folder this app never deletes from',
  );
  // The one entry that is going, and its size is the whole of what the batch may free.
  const promised = await printedSize(total);
  await dialog.getByRole('button', { name: 'Move to the Trash' }).click();

  await expect(dialog.getByTestId('result-summary')).toHaveText(
    `Moved 1 item to the Trash · ${promised}`,
  );
  await expect(dialog.getByTestId('result-skipped')).toContainText(`${ROOT}/Library`);
  await dialog.getByRole('button', { name: 'Close' }).click();

  // Library is still here, and it is the row the guards refused rather than one the table
  // happens to have redrawn: the batch took the other one with it.
  await expect(names(page)).toHaveText([
    'Library',
    'Downloads',
    'Pictures',
    'src',
    'Documents',
    '.zshrc',
    '.Trash',
    'OrbStack',
  ]);
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

  /**
   * `explorer-selection.png`: the one gate on what the two danger styles *look* like.
   *
   * Which variant each button wears is pinned by a test (`data-variant`), and that is as
   * far as a unit test reaches — that red reads as red, and that the bar's red entry point
   * and the dialog's red confirm do not make a wall of the corner they share, is a
   * question only a picture answers. So the frame has all three in it: the bar's "Move to
   * Trash" and "Delete permanently" behind the backdrop, and the dialog armed for the
   * deletion that cannot be taken back, acknowledgement ticked so the button is live.
   *
   * It was this picture that showed the two reds carrying the same words as well as the
   * same variant; the dialog's now says "Delete for good", and the assertion below is by
   * name so that this test holds them apart as well as showing them.
   *
   * Not `fullPage`: the backdrop is `fixed`, and a full-page shot of a fixed layer past the
   * fold is a picture of the layout coming apart rather than of the app.
   */
  test('explorer-selection.png (the dialog armed for a permanent deletion)', async ({ page }) => {
    await page.emulateMedia({ colorScheme: 'light' });
    await scan(page);
    await box(page, 'Downloads').check();
    await box(page, 'Movies').check();

    await bar(page).getByRole('button', { name: 'Delete permanently' }).click();
    const dialog = page.getByRole('dialog');
    await expect(dialog.getByRole('radio', { name: 'Permanent' })).toBeChecked();
    await dialog.getByRole('checkbox', { name: /I understand/ }).check();
    const confirm = dialog.getByRole('button', { name: 'Delete for good' });
    await expect(confirm).toHaveAttribute('data-variant', 'danger');
    await expect(confirm).toBeEnabled();
    // Reached by name rather than by test id, so that the shot is taken of a screen whose
    // two red buttons say different things — which is the point of the shot.
    await expect(bar(page).getByRole('button', { name: 'Delete permanently' })).toBeVisible();

    await page.screenshot({ path: test.info().outputPath('explorer-selection.png') });
  });
});
