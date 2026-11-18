import { expect, test } from '@playwright/test';

import { mockGateway } from '../fixtures';

const publicDocs = 'https://agentgateway.dev/docs/standalone/latest/';
const runtimeInfo = (loaded: boolean) => ({
	build: {},
	ui: {
		gatewayMode: 'standalone',
		configStoreMode: 'file',
		...(loaded ? { documentationPath: '/ui/docs/guide/' } : {})
	}
});

for (const loaded of [false, true]) {
	test(`Documentation footer icon: bundle ${loaded ? 'loaded' : 'absent'}`, async ({ page }) => {
		await mockGateway(page);
		await page.route('**/api/runtime', route => route.fulfill({ json: runtimeInfo(loaded) }));
		await page.goto('/');
		const docs = page.locator('.sidebar-links').getByRole('link', { name: 'Documentation' });
		await expect(docs).toBeVisible();
		await expect(docs).toHaveAttribute('href', loaded ? '/documentation' : publicDocs);
		await expect(
			page.locator('.nav-list').getByRole('link', { name: 'Docs', exact: true })
		).toHaveCount(0);
		if (!loaded) await expect(docs).toHaveAttribute('target', '_blank');
	});
}

test('Local documentation navigation keeps the gateway sidebar visible', async ({ page }) => {
	await mockGateway(page);
	await page.route('**/api/runtime', route => route.fulfill({ json: runtimeInfo(true) }));
	await page.route('**/ui/docs/guide/**', route =>
		route.fulfill({
			contentType: 'text/html',
			body: route.request().url().endsWith('/next/')
				? '<h1>Next article</h1>'
				: '<h1>Local guide</h1><a href="/ui/docs/guide/next/">Next article</a>'
		})
	);
	await page.goto('/');
	const icon = page.getByRole('link', { name: 'Documentation' });
	await expect(icon).toHaveAttribute('href', '/documentation');
	await icon.click();
	await expect(page).toHaveURL(/\/documentation$/);
	const docs = page.frameLocator('iframe[title="Documentation"]');
	await expect(docs.getByRole('heading', { name: 'Local guide' })).toBeVisible();
	await expect(page.locator('.breadcrumb')).toContainText('Documentation');
	await docs.getByRole('link', { name: 'Next article' }).click();
	await expect(docs.getByRole('heading', { name: 'Next article' })).toBeVisible();
	await expect(page.locator('.sidebar')).toBeVisible();
	await expect(page).toHaveURL(/\/documentation$/);
	await page.reload();
	await expect(docs.getByRole('heading', { name: 'Local guide' })).toBeVisible();
	await page.locator('.nav-list').getByRole('link', { name: 'Home', exact: true }).click();
	await expect(page.locator('iframe[title="Documentation"]')).toHaveCount(0);
});

test('Documentation uses the public link until runtime confirms a loaded bundle', async ({
	page
}) => {
	await mockGateway(page);
	let release!: () => void;
	const waiting = new Promise<void>(resolve => {
		release = resolve;
	});
	await page.route('**/api/runtime', async route => {
		await waiting;
		await route.fulfill({ json: runtimeInfo(true) });
	});
	await page.goto('/');
	const docs = page.getByRole('link', { name: 'Documentation' });
	await expect(docs).toHaveAttribute('href', publicDocs);
	release();
	await expect(docs).toHaveAttribute('href', '/documentation');
});

test('Documentation view offers the public website when no bundle is available', async ({
	page
}) => {
	await mockGateway(page);
	await page.route('**/api/runtime', route => route.fulfill({ json: runtimeInfo(false) }));
	await page.goto('/documentation');
	await expect(page.getByRole('link', { name: 'Open public documentation' })).toHaveAttribute(
		'href',
		publicDocs
	);
	await expect(page.locator('iframe[title="Documentation"]')).toHaveCount(0);
	await expect(page.locator('.sidebar')).toBeVisible();
});
