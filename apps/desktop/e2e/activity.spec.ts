// The Activity screen against the mocked backend: the record of a batch that this session
// ran, read back from the log.
//
// The batch has to run in the same page session as the reading. The mock's log lives in the
// module, so every `page.goto` starts with an empty one — which is what the second test uses
// and what would quietly empty the first.

import { expect, test, type Locator, type Page } from './fixtures';

const ROOT = '/Users/demo';
const SHIELDED = 'Not as a whole — open it and choose what inside';

// Tall enough for the whole table, like the Explorer's own screenshots.
test.use({ viewport: { width: 1280, height: 900 } });

function rows(page: Page): Locator {
  return page.getByTestId('activity-rows').getByRole('row');
}

/** The path of each row: the span carrying a title. The detail under it carries none. */
function paths(page: Page): Locator {
  return page.getByTestId('activity-rows').locator('span[title]');
}

/** The cells of one row, in the order of the headers: When, What, Mode, Result, Size. */
function cells(page: Page, index: number): Locator {
  return rows(page).nth(index).getByRole('cell');
}

function open(page: Page, section: string): Promise<void> {
  return page
    .getByRole('navigation', { name: 'Sections' })
    .getByRole('button', { name: section })
    .click();
}

/**
 * Scans, ticks three rows of the root and moves them to the Trash. `Library` is shielded, so
 * the batch is the interesting one: two entries leave and the third is refused.
 */
async function deleteThree(page: Page): Promise<void> {
  await page.goto('/');
  await page.getByRole('button', { name: 'Scan', exact: true }).click();
  await expect(page.getByRole('table')).toBeVisible();
  for (const name of ['Library', 'Downloads', 'Movies']) {
    await page
      .getByTestId('node-rows')
      .locator('tr')
      .filter({ has: page.locator(`span[title="${name}"]`) })
      .getByRole('checkbox')
      .check();
  }
  await page.getByTestId('selection-bar').getByRole('button', { name: 'Move to Trash' }).click();
  const dialog = page.getByRole('dialog');
  await dialog.getByRole('button', { name: 'Move to the Trash' }).click();
  await expect(dialog.getByTestId('result-summary')).toBeVisible();
  await dialog.getByRole('button', { name: 'Close' }).click();
  await expect(dialog).toBeHidden();
}

test('records what the batch did, newest first, and says why the third was refused', async ({
  page,
}) => {
  await deleteThree(page);
  await open(page, 'Activity');

  await expect(page.getByRole('heading', { name: 'Activity' })).toBeVisible();
  await expect(page.getByTestId('activity-summary')).toHaveText('3 entries');
  await expect(page.getByRole('columnheader')).toHaveText([
    'When',
    'What',
    'Mode',
    'Result',
    'Size',
  ]);

  // Newest first, which inside one batch is the order of the batch reversed: the entries
  // were sent in the order the table showed them.
  await expect(paths(page)).toHaveText([`${ROOT}/Movies`, `${ROOT}/Downloads`, `${ROOT}/Library`]);
  await expect(rows(page).nth(0)).toHaveAttribute('data-result', 'removed');
  await expect(rows(page).nth(1)).toHaveAttribute('data-result', 'removed');
  await expect(rows(page).nth(2)).toHaveAttribute('data-result', 'skipped');

  // The batch stamped itself a moment ago, in this browser's own zone, so the shape is all
  // a test that has to run anywhere can say about it.
  await expect(cells(page, 0).nth(0)).toHaveText(/^\d{4}-\d{2}-\d{2} \d{2}:\d{2}:\d{2}$/);
  // A removed row carries the mode it left by and the bytes it took with it.
  await expect(cells(page, 0).nth(2)).toHaveText('Trash');
  await expect(cells(page, 0).nth(3)).toHaveText('Removed');
  await expect(cells(page, 0).nth(4)).toHaveText('22.1 GB');
  await expect(cells(page, 1).nth(4)).toHaveText('28.7 GB');

  // The refused one says nothing it cannot stand behind: no mode, because nothing moved
  // anywhere, and no size, because nothing was freed. It does say why.
  await expect(cells(page, 2).nth(2)).toHaveText('—');
  await expect(cells(page, 2).nth(3)).toHaveText('Skipped');
  await expect(cells(page, 2).nth(4)).toHaveText('—');
  const reason = rows(page).nth(2).getByTestId('activity-detail');
  await expect(reason).toHaveText(SHIELDED);
  await expect(reason).toHaveAttribute('data-detail', 'reason');
  await expect(page.getByTestId('activity-damaged')).toBeHidden();

  await page.screenshot({ path: test.info().outputPath('activity.png'), fullPage: true });
});

test('says so plainly when nothing has been deleted yet', async ({ page }) => {
  await page.goto('/');
  await open(page, 'Activity');

  await expect(page.getByTestId('activity-empty')).toContainText('No actions yet');
  await expect(page.getByTestId('activity-rows')).toBeHidden();
  await expect(page.getByTestId('activity-summary')).toBeHidden();

  // And back out of it, which is the other half of a section being reachable at all. The
  // Explorer is at its empty state because nothing has scanned in this session.
  await open(page, 'Explorer');
  await expect(page.getByRole('button', { name: 'Scan', exact: true })).toBeVisible();
});
