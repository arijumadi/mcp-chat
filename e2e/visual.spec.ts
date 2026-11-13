import { expect, test } from "@playwright/test";

// Visual baselines are intentionally limited to stable, fully mocked screens.
// Update with `pnpm exec playwright test e2e/visual.spec.ts --update-snapshots`.
test.describe("visual regression", () => {
  test("chat transcript", async ({ page }) => {
    await page.goto("/?mock=1");
    await page.getByText("Welcome to Jarvis").first().click();
    await expect(page.getByText("What can you do with MCP tools?")).toBeVisible();
    await page.evaluate(() => (document.activeElement as HTMLElement | null)?.blur());
    await page.mouse.move(10, 10);

    await expect(page).toHaveScreenshot("chat.png", {
      animations: "disabled",
      maxDiffPixelRatio: 0.01,
    });
  });

  test("settings providers", async ({ page }) => {
    await page.goto("/?mock=1");
    await page.getByRole("complementary").getByRole("button", { name: "Settings" }).click();
    await expect(page.getByRole("heading", { name: "Providers" })).toBeVisible();
    await expect(page.getByText("Mock provider")).toBeVisible();
    await page.evaluate(() => (document.activeElement as HTMLElement | null)?.blur());
    await page.mouse.move(10, 10);

    await expect(page).toHaveScreenshot("settings.png", {
      animations: "disabled",
      maxDiffPixelRatio: 0.01,
    });
  });

  test("mcp servers", async ({ page }) => {
    await page.goto("/?mock=1");
    await page.getByRole("complementary").getByRole("button", { name: "MCP servers" }).click();
    await page.getByRole("button", { name: "Show tools" }).click();
    await expect(page.getByText("Echo text back")).toBeVisible();
    await page.evaluate(() => (document.activeElement as HTMLElement | null)?.blur());
    await page.mouse.move(10, 10);

    await expect(page).toHaveScreenshot("servers.png", {
      animations: "disabled",
      maxDiffPixelRatio: 0.01,
    });
  });

  test("skills", async ({ page }) => {
    await page.goto("/?mock=1");
    await page.getByRole("complementary").getByRole("button", { name: "Skills" }).click();
    await expect(page.getByText("Global skill settings")).toBeVisible();
    await expect(page.getByText("excalidraw").first()).toBeVisible();
    await page.evaluate(() => (document.activeElement as HTMLElement | null)?.blur());
    await page.mouse.move(10, 10);

    await expect(page).toHaveScreenshot("skills.png", {
      animations: "disabled",
      maxDiffPixelRatio: 0.01,
    });
  });
});

test.describe("minimum window width", () => {
  test.use({ viewport: { width: 860, height: 860 } });

  test("empty chat stays usable", async ({ page }) => {
    await page.goto("/?mock=1");
    await expect(page.getByText("A quiet place to talk to your tools.")).toBeVisible();
    await expect(page.getByRole("button", { name: "Map the workspace" })).toBeVisible();
    await page.evaluate(() => (document.activeElement as HTMLElement | null)?.blur());
    await page.mouse.move(10, 10);

    await expect(page).toHaveScreenshot("chat-narrow.png", {
      animations: "disabled",
      maxDiffPixelRatio: 0.01,
    });
  });

  test("settings stays inside the viewport", async ({ page }) => {
    await page.goto("/?mock=1");
    await page.getByRole("complementary").getByRole("button", { name: "Settings" }).click();
    await expect(page.getByRole("heading", { name: "Providers" })).toBeVisible();

    const overflows = await page.evaluate(
      () => document.documentElement.scrollWidth > document.documentElement.clientWidth,
    );
    expect(overflows).toBe(false);
  });
});

test.describe("narrow chat", () => {
  test.use({ viewport: { width: 860, height: 860 } });

  test("composer and transcript fit", async ({ page }) => {
    await page.goto("/?mock=1");
    await page.getByText("Welcome to Jarvis").first().click();
    await expect(page.getByText("What can you do with MCP tools?")).toBeVisible();

    const overflows = await page.evaluate(
      () => document.documentElement.scrollWidth > document.documentElement.clientWidth,
    );
    expect(overflows).toBe(false);
    await page.evaluate(() => (document.activeElement as HTMLElement | null)?.blur());
    await page.mouse.move(10, 10);

    await expect(page).toHaveScreenshot("chat-composer-narrow.png", {
      animations: "disabled",
      maxDiffPixelRatio: 0.01,
    });
  });
});
