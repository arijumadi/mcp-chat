import { expect, test } from "@playwright/test";

test("boots, sends a scripted message and renders a tool card", async ({ page }) => {
  await page.goto("/?mock=1");
  await expect(page.getByText("MCP Chat").first()).toBeVisible();
  await expect(page.getByText("Welcome to MCP Chat").first()).toBeVisible();

  await page.getByRole("button", { name: "New chat" }).first().click();
  const composer = page.getByRole("textbox", { name: "Message" });
  await expect(composer).toBeVisible();
  await composer.fill("Hello from Playwright");
  await composer.press("Enter");

  await expect(page.getByText(/I received/)).toBeVisible({ timeout: 15_000 });
  await expect(
    page.getByRole("button", { name: /Used 1 tool/ }),
  ).toBeVisible();
  await page.getByRole("button", { name: /Used 1 tool/ }).click();
  await expect(page.getByText("Mock MCP").first()).toBeVisible();
  await expect(page.getByText("ok").first()).toBeVisible({ timeout: 15_000 });
  await expect(page.getByRole("button", { name: "Send message" })).toBeVisible({
    timeout: 15_000,
  });
  await expect(page.getByText(/I received/)).toBeVisible();
});

test("quick switcher finds the demo conversation", async ({ page }) => {
  await page.goto("/?mock=1");
  await page.keyboard.press("Control+k");
  const input = page.getByPlaceholder("Search conversations…");
  await expect(input).toBeVisible();
  await input.fill("Welcome");
  await expect(page.getByRole("dialog").getByText("Welcome to MCP Chat")).toBeVisible();
  await page.keyboard.press("Enter");
  await expect(page.getByText("What can you do with MCP tools?")).toBeVisible();
});

test("settings and MCP screens render", async ({ page }) => {
  await page.goto("/?mock=1");
  await page.getByRole("complementary").getByRole("button", { name: "Settings" }).click();
  await expect(page.getByRole("heading", { name: "Providers" })).toBeVisible();
  await expect(page.getByText("Mock provider")).toBeVisible();

  await page.getByRole("complementary").getByRole("button", { name: "MCP servers" }).click();
  await expect(page.getByText("Mock MCP").first()).toBeVisible();
  await page.getByRole("button", { name: "Tools" }).click();
  await expect(page.getByText("Echo text back")).toBeVisible();
});


test("revert moves a user message back into the composer", async ({ page }) => {
  await page.goto("/?mock=1");
  await page.getByRole("button", { name: "New chat" }).first().click();
  const composer = page.getByRole("textbox", { name: "Message" });
  await composer.fill("Edit this message");
  await composer.press("Enter");
  await expect(page.getByRole("button", { name: "Send message" })).toBeVisible({
    timeout: 15_000,
  });
  await expect(page.getByText(/I received/)).toBeVisible();

  await page.getByTestId("user-message").last().hover();
  await page.getByRole("button", { name: "Revert message" }).click();

  await expect(composer).toHaveValue("Edit this message");
  await expect(page.getByText(/I received/)).toHaveCount(0);
  await expect(page.getByTestId("user-message")).toHaveCount(0);
});

test("captures README screenshots", async ({ page }) => {
  await page.goto("/?mock=1");
  await page.getByText("Welcome to MCP Chat").first().click();
  await expect(page.getByText("What can you do with MCP tools?")).toBeVisible();
  await page.getByRole("button", { name: /Tools/ }).click();
  await expect(page.getByText("MCP servers for this chat")).toBeVisible();
  await expect(page.getByText("Context estimate")).toBeVisible();
  await page.getByRole("button", { name: /Tools/ }).click();
  await page.getByText("What can you do with MCP tools?").click();
  await page.evaluate(() => (document.activeElement as HTMLElement | null)?.blur());
  await page.mouse.move(10, 10);
  await page.screenshot({ path: "docs/screenshots/chat.png", fullPage: false });

  await page.getByRole("complementary").getByRole("button", { name: "Settings" }).click();
  await expect(page.getByRole("heading", { name: "Providers" })).toBeVisible();
  await page.screenshot({ path: "docs/screenshots/settings.png", fullPage: false });

  await page.getByRole("complementary").getByRole("button", { name: "MCP servers" }).click();
  await page.getByRole("button", { name: "Tools" }).click();
  await expect(page.getByText("Echo text back")).toBeVisible();
  await page.screenshot({ path: "docs/screenshots/servers.png", fullPage: false });
});
