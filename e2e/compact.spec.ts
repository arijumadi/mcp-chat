import { expect, test, type Page } from "@playwright/test";

async function openDemoWithHistory(page: Page) {
  await page.goto("/?mock=1");
  await page.getByText("Welcome to Jarvis").first().click();
  await expect(page.getByText("What can you do with MCP tools?")).toBeVisible();
  // The demo chat has three messages; compaction needs at least four.
  const composer = page.getByRole("textbox", { name: "Message" });
  await composer.fill("one more turn");
  await composer.press("Enter");
  await expect(page.getByRole("button", { name: "Send message" })).toBeVisible({ timeout: 15_000 });
  return composer;
}

test("/compact asks first, then compacts and shows the summary", async ({ page }) => {
  const composer = await openDemoWithHistory(page);
  await composer.fill("/compact keep the tool results");
  await composer.press("Enter");

  const dialog = page.getByRole("dialog", { name: /Compact this conversation/ });
  await expect(dialog).toBeVisible();
  await expect(dialog.getByText(/This cannot be undone/)).toBeVisible();
  await expect(dialog.getByLabel("Focus (optional)")).toHaveValue("keep the tool results");
  // Nothing was sent to the model and nothing has changed yet.
  await expect(page.getByTestId("user-message").filter({ hasText: "/compact" })).toHaveCount(0);
  await expect(page.getByText("Earlier conversation summary")).toHaveCount(0);

  await dialog.getByRole("button", { name: "Compact" }).click();
  await expect(dialog).toHaveCount(0);
  await expect(page.getByText("Conversation compacted.")).toBeVisible();
  const summary = page.getByText("Earlier conversation summary");
  await expect(summary).toBeVisible();
  await summary.click();
  await expect(page.getByText(/Focus: keep the tool results/)).toBeVisible();
  await expect(composer).toHaveValue("");
});

test("cancelling /compact keeps what was typed", async ({ page }) => {
  const composer = await openDemoWithHistory(page);
  await composer.fill("/compact");
  await composer.press("Enter");
  const dialog = page.getByRole("dialog", { name: /Compact this conversation/ });
  await dialog.getByRole("button", { name: "Cancel" }).click();
  await expect(dialog).toHaveCount(0);
  await expect(composer).toHaveValue("/compact");
  await expect(page.getByText("Earlier conversation summary")).toHaveCount(0);
});

test("a chat that is too short explains why it cannot be compacted", async ({ page }) => {
  await page.goto("/?mock=1");
  await page.getByRole("button", { name: "New chat" }).first().click();
  const composer = page.getByRole("textbox", { name: "Message" });
  await composer.fill("/compact");
  await composer.press("Enter");
  await page.getByRole("dialog").getByRole("button", { name: "Compact" }).click();
  await expect(page.getByText(/Not enough history to compact/)).toBeVisible();
});
