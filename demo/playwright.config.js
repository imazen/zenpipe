// @ts-check
const { defineConfig } = require('@playwright/test');

module.exports = defineConfig({
  testDir: './tests',
  timeout: 30_000,
  expect: { timeout: 10_000 },
  use: {
    baseURL: 'http://localhost:3847',
    screenshot: 'only-on-failure',
  },
  webServer: {
    command: 'python3 -m http.server 3847',
    port: 3847,
    cwd: __dirname,
    reuseExistingServer: true,
  },
  projects: [
    { name: 'chromium', use: { browserName: 'chromium' } },
  ],
});
