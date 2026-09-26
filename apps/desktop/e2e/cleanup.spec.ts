// The Cleanup screen against the mocked backend: the demo module of a debug build,
// discovered, cleaned in both modes, and read back in Activity — the exit criterion of phase
// 2b, as far as a browser can reach it.
//
// The mock's world lives in the module, so every `page.goto` starts with a sandbox seeded
// again and an empty log: a batch and its reading have to happen in one page session.

import { expect, test, type Locator, type Page } from './fixtures';

const SANDBOX = '/Users/demo/Library/Application Support/storage-monitor/demo';

// Tall enough for the table, the detail panel and the dialog at once.
test.use({ viewport: { width: 1280, height: 900 } });

function open(page: Page, section: string): Promise<void> {
  return page
    .getByRole('navigation', { name: 'Sections' })
    .getByRole('button', { name: section })
    .click();
}

/** The table's rows, by the title each carries. */
function row(page: Page, title: string): Locator {
  return page
    .getByTestId('item-rows')
    .getByRole('row')
    .filter({ has: page.locator(`span[title="${title}"]`) });
}

/** The titles of the rows, in the order the table shows them. */
function titles(page: Page): Locator {
  return page.getByTestId('item-rows').locator('tr span[title]');
}

function box(page: Page, title: string): Locator {
  return row(page, title).getByRole('checkbox');
}

function dialog(page: Page): Locator {
  return page.getByRole('dialog');
}

/** Opens Cleanup and waits for the demo module to have looked around. */
async function cleanup(page: Page): Promise<void> {
  await page.goto('/');
  await open(page, 'Cleanup');
  await expect(page.getByTestId('module-demo')).toHaveAttribute('data-status', 'ready');
}

test('discovers the demo module and lists what it found, Keep left out', async ({ page }) => {
  await cleanup(page);
  await expect(page.getByTestId('module-demo')).toContainText('5 items · 7.7 MB');
  await expect(titles(page)).toHaveText(['fresh.object', 'build-cache', 'logs', 'old.object']);
  await expect(row(page, 'fresh.object')).toContainText('~3.0 MB');
  await page.getByRole('button', { name: /Keep/ }).click();
  await expect(titles(page)).toHaveText([
    'fresh.object',
    'build-cache',
    'logs',
    'old.object',
    'keepsake',
  ]);
});

test('cleans a folder and an object in Trash mode, and Activity records both', async ({ page }) => {
  await cleanup(page);
  await box(page, 'build-cache').check();
  await box(page, 'old.object').check();
  await expect(page.getByTestId('cleanup-bar')).toContainText('2 items selected · 3.0 MB');
  await page.getByTestId('cleanup-bar').getByRole('button', { name: 'Clean…' }).click();

  await expect(dialog(page)).toHaveAccessibleName('Clean 2 items?');
  await expect(dialog(page)).toContainText(`Move to the Trash${SANDBOX}/build-cache`);
  await expect(dialog(page)).toContainText(`rm '${SANDBOX}/old.object'`);
  // The object is removed by a command in either mode: the Trash cannot undo it.
  const confirm = dialog(page).getByTestId('confirm-delete');
  await expect(confirm).toBeDisabled();
  await dialog(page)
    .getByRole('checkbox', { name: /1 item cannot be undone/ })
    .check();
  await confirm.click();

  await expect(dialog(page).getByTestId('result-summary')).toHaveText(
    'Deleted 1 item and moved 1 to the Trash · 3.0 MB',
  );
  await dialog(page).getByRole('button', { name: 'Close' }).click();
  await expect(titles(page)).toHaveText(['fresh.object', 'logs']);

  await open(page, 'Activity');
  const lines = page.getByTestId('activity-source');
  await expect(lines).toHaveText(['Demo · Remove object', 'Demo · Delete folder']);
  await expect(page.getByTestId('activity-command').first()).toContainText(
    `$ rm '${SANDBOX}/old.object'`,
  );
});

test('holds a permanent batch until the acknowledgement', async ({ page }) => {
  await cleanup(page);
  await box(page, 'logs').check();
  await page.getByTestId('cleanup-bar').getByRole('button', { name: 'Clean…' }).click();
  await dialog(page).getByRole('radio', { name: 'Permanent' }).check();
  await expect(dialog(page)).toHaveAccessibleName('Clean 1 item permanently?');
  await expect(dialog(page)).toContainText(`Delete${SANDBOX}/logs`);
  const confirm = dialog(page).getByTestId('confirm-delete');
  await expect(confirm).toHaveText('Clean for good');
  await expect(confirm).toBeDisabled();
  await dialog(page)
    .getByRole('checkbox', { name: /this cannot be undone/ })
    .check();
  await confirm.click();
  await expect(dialog(page).getByTestId('result-summary')).toHaveText('Deleted 1 item · 1.2 MB');
});

test('refuses the Keep row until its force option is on', async ({ page }) => {
  await cleanup(page);
  await page.getByRole('button', { name: /Keep/ }).click();
  await row(page, 'keepsake').click();
  const detail = page.getByTestId('item-detail');
  await expect(detail.getByTestId('reasons')).toHaveText('It holds a KEEP file');

  await detail.getByRole('button', { name: 'Clean…' }).click();
  await expect(dialog(page).getByTestId('block-reason')).toHaveText(
    'Marked keep — turn on its force option to clean it anyway',
  );
  await expect(dialog(page).getByTestId('confirm-delete')).toBeDisabled();
  await dialog(page).getByRole('button', { name: 'Cancel' }).click();

  await detail.getByRole('checkbox', { name: /Delete it although it is marked keep/ }).check();
  await detail.getByRole('button', { name: 'Clean…' }).click();
  await expect(dialog(page).getByTestId('block-reason')).toHaveCount(0);
  await dialog(page).getByTestId('confirm-delete').click();
  await expect(dialog(page).getByTestId('result-summary')).toHaveText(
    'Moved 1 item to the Trash · 507.9 KB',
  );
});

test.describe('screenshots for the PR', () => {
  test('cleanup.png (ticked rows and the detail panel)', async ({ page }) => {
    await cleanup(page);
    await box(page, 'build-cache').check();
    await box(page, 'old.object').check();
    await row(page, 'build-cache').click();
    await expect(page.getByTestId('item-detail')).toBeVisible();
    await page.screenshot({ path: test.info().outputPath('cleanup.png'), fullPage: true });
  });

  test('cleanup-dialog.png (a batch in Trash mode that still asks)', async ({ page }) => {
    await cleanup(page);
    await box(page, 'build-cache').check();
    await box(page, 'old.object').check();
    await page.getByTestId('cleanup-bar').getByRole('button', { name: 'Clean…' }).click();
    await expect(dialog(page)).toBeVisible();
    await page.screenshot({ path: test.info().outputPath('cleanup-dialog.png') });
  });
});
