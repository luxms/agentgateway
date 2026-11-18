import { expect, test } from '@playwright/test';

import { mockGateway } from '../fixtures';

for (const loaded of [false, true]) {
	for (const mobile of [false, true]) {
		test(`Docs navigation: bundle ${loaded ? 'loaded' : 'absent'}, ${mobile ? 'mobile' : 'desktop'}`, async ({
			page
		}) => {
			if (mobile) await page.setViewportSize({ width: 390, height: 844 });
			await mockGateway(page);
			await page.route('**/api/runtime', route =>
				route.fulfill({
					json: {
						build: {},
						ui: {
							gatewayMode: 'standalone',
							configStoreMode: 'file',
							...(loaded ? { documentationPath: '/ui/docs/guide/' } : {})
						}
					}
				})
			);
			const runtime = page.waitForResponse('**/api/runtime');
			await page.goto('/');
			await runtime;
			if (mobile) await page.locator('.mobile-nav-trigger').click();
			const navigation = page.locator(mobile ? '.mobile-nav-menu' : '.nav-list');
			const docs = navigation.getByRole('link', { name: 'Docs', exact: true });
			if (loaded) {
				await expect(docs).toBeVisible();
				await expect(docs).toHaveAttribute('href', '/ui/docs/guide/');
				await expect(docs).toHaveAttribute('target', '_blank');
			} else {
				await expect(page.getByRole('link', { name: 'Docs', exact: true })).toHaveCount(0);
			}
			await expect(page.getByRole('link', { name: 'Documentation', exact: true })).toHaveCount(0);
		});
	}
}

test('Docs stays hidden until runtime confirms a loaded bundle', async ({ page }) => {
	await mockGateway(page);
	let release!: () => void;
	const waiting = new Promise<void>(resolve => {
		release = resolve;
	});
	await page.route('**/api/runtime', async route => {
		await waiting;
		await route.fulfill({
			json: {
				build: {},
				ui: { gatewayMode: 'standalone', configStoreMode: 'file', documentationPath: '/ui/docs/' }
			}
		});
	});
	await page.goto('/');
	await expect(page.getByRole('link', { name: 'Docs', exact: true })).toHaveCount(0);
	release();
	await expect(page.getByRole('link', { name: 'Docs', exact: true })).toBeVisible();
});
