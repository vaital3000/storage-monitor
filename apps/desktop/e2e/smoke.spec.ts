import { expect, test } from './fixtures';

test('renders the app name and the mocked backend version', async ({ page }) => {
  await page.goto('/');
  await expect(page.getByRole('heading', { name: 'Storage Monitor' })).toBeVisible();
  await expect(page.getByTestId('version')).toHaveText('v0.0.0-mock');
});

test('takes a screenshot for the PR', async ({ page }) => {
  await page.goto('/');
  await expect(page.getByTestId('version')).toHaveText('v0.0.0-mock');
  await page.screenshot({ path: test.info().outputPath('home.png'), fullPage: true });
});
