import { expect, test, type Page } from "@playwright/test";

async function openDemo(page: Page) {
  await page.goto("/?mock=1");
  await page.getByText("Welcome to Jarvis").first().click();
  await expect(page.getByText("What can you do with MCP tools?")).toBeVisible();
}

async function openAppearance(page: Page) {
  await page.getByRole("complementary").getByRole("button", { name: "Settings" }).click();
  await expect(page.getByRole("heading", { name: "Appearance" })).toBeVisible();
}

async function backToDemo(page: Page) {
  await page.getByText("Welcome to Jarvis").first().click();
  await expect(page.getByText("What can you do with MCP tools?")).toBeVisible();
}

/** Size of the first message-sized text, and the height of the first activity row. */
const measure = (page: Page) =>
  page.evaluate(() => {
    const prose = document.querySelector(".md") as HTMLElement;
    const row = document.querySelector("[data-tool-name] button") as HTMLElement;
    const transcript = document.querySelector(".md")!.closest("[class*='max-w-']") as HTMLElement;
    return {
      fontSize: parseFloat(getComputedStyle(prose).fontSize),
      rowHeight: row.getBoundingClientRect().height,
      rowFont: parseFloat(getComputedStyle(row).fontSize),
      width: transcript.getBoundingClientRect().width,
      scrollOverflow: document.documentElement.scrollWidth > document.documentElement.clientWidth,
    };
  });

test("the defaults are the original look", async ({ page }) => {
  await openDemo(page);
  const m = await measure(page);
  expect(m.fontSize).toBe(15);
  // Interactive rows have always rendered at the base chat size (15px).
  expect(m.rowFont).toBe(15);
  expect(m.width).toBeLessThanOrEqual(760);
  expect(await page.evaluate(() => document.documentElement.dataset.density)).toBe("comfortable");
});

test("compact density makes the transcript tighter without changing the text", async ({ page }) => {
  await openDemo(page);
  const before = await measure(page);
  await openAppearance(page);
  await page.getByRole("radio", { name: "Compact" }).click();
  await backToDemo(page);
  const after = await measure(page);
  expect(after.rowHeight).toBeLessThan(before.rowHeight);
  expect(after.fontSize).toBe(before.fontSize);
  expect(after.rowFont).toBe(before.rowFont);
});

test("the text size slider scales message text and tool rows together", async ({ page }) => {
  await openDemo(page);
  await openAppearance(page);
  const slider = page.getByRole("slider", { name: "Chat text size" });
  await slider.focus();
  for (let i = 0; i < 3; i += 1) await slider.press("ArrowRight");
  await expect(page.getByText("18px")).toBeVisible();
  await backToDemo(page);
  const m = await measure(page);
  expect(m.fontSize).toBe(18);
  // The row follows the slider too; before this it ignored the chat size.
  expect(m.rowFont).toBe(18);
});

test("the width setting changes how wide the transcript can grow", async ({ page }) => {
  await openDemo(page);
  const base = (await measure(page)).width;
  await openAppearance(page);
  await page.getByRole("radio", { name: "Narrow" }).click();
  await backToDemo(page);
  const narrow = (await measure(page)).width;
  await openAppearance(page);
  await page.getByRole("radio", { name: "Full" }).click();
  await backToDemo(page);
  const full = (await measure(page)).width;
  expect(narrow).toBeLessThan(base);
  expect(full).toBeGreaterThan(base);
});

test("settings are remembered after a reload", async ({ page }) => {
  await openDemo(page);
  await openAppearance(page);
  await page.getByRole("radio", { name: "Compact" }).click();
  await page.getByRole("radio", { name: "Wide" }).click();
  await page.getByRole("slider", { name: "Chat text size" }).fill("17");

  await page.reload();
  await expect(page.getByText("Welcome to Jarvis").first()).toBeVisible();
  expect(await page.evaluate(() => document.documentElement.dataset.density)).toBe("compact");
  expect(await page.evaluate(() => document.documentElement.style.getPropertyValue("--chat-fs"))).toBe("17px");
  expect(await page.evaluate(() => document.documentElement.style.getPropertyValue("--chat-width"))).toBe("960px");
});

test("reset puts everything back", async ({ page }) => {
  await openDemo(page);
  await openAppearance(page);
  await page.getByRole("radio", { name: "Compact" }).click();
  await page.getByRole("radio", { name: "Full" }).click();
  await page.getByRole("button", { name: "Reset to defaults" }).click();
  await expect(page.getByRole("radio", { name: "Comfortable" })).toHaveAttribute("aria-checked", "true");
  await expect(page.getByRole("radio", { name: "Default" })).toHaveAttribute("aria-checked", "true");
  await expect(page.getByText("15px")).toBeVisible();
});

test.describe("minimum window width", () => {
  test.use({ viewport: { width: 860, height: 860 } });

  test("the largest text and the widest chat still fit", async ({ page }) => {
    await openDemo(page);
    await openAppearance(page);
    await page.getByRole("slider", { name: "Chat text size" }).fill("19");
    await page.getByRole("radio", { name: "Full" }).click();
    await backToDemo(page);
    expect((await measure(page)).scrollOverflow).toBe(false);
    // Expanding rows at the largest size must not overflow either.
    await page.getByRole("button", { name: /Mock MCP\/echo/ }).click();
    await page.locator('[data-tool-name="edit"] button').first().click();
    expect((await measure(page)).scrollOverflow).toBe(false);
  });
});

test("the shortcut list reflects the command registry", async ({ page }) => {
  await openDemo(page);
  await page.getByRole("complementary").getByRole("button", { name: "Settings" }).click();
  const list = page.getByRole("heading", { name: "Keyboard shortcuts" });
  await expect(list).toBeVisible();
  await expect(page.getByText("Compact conversation")).toBeVisible();
  await expect(page.getByText("Ctrl+Shift+P").first()).toBeVisible();
});
