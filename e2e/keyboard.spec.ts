import { expect, test, type Page } from "@playwright/test";

const MOD = "Control";

async function openChat(page: Page) {
  await page.goto("/?mock=1");
  await page.getByText("Welcome to Jarvis").first().click();
  await expect(page.getByText("What can you do with MCP tools?")).toBeVisible();
}

async function askForApproval(page: Page) {
  const composer = page.getByRole("textbox", { name: "Message" });
  await composer.fill("this needs approval");
  await composer.press("Enter");
  const dialog = page.getByRole("dialog", { name: /Tool permission/ });
  await expect(dialog).toBeVisible();
  return { composer, dialog };
}

const palette = (page: Page) => page.getByRole("dialog", { name: "Command palette" });

test.describe("command palette", () => {
  test("opens with Ctrl+Shift+P, filters, and runs a command", async ({ page }) => {
    await openChat(page);
    await page.keyboard.press(`${MOD}+Shift+P`);
    await expect(palette(page)).toBeVisible();
    const search = palette(page).getByRole("combobox");
    await expect(search).toBeFocused();

    await search.fill("settings");
    await expect(palette(page).getByRole("option")).toHaveCount(1);
    await search.press("Enter");
    await expect(palette(page)).toHaveCount(0);
    await expect(page.getByRole("heading", { name: "Providers" })).toBeVisible();
  });

  test("lists the shortcut next to each command", async ({ page }) => {
    await openChat(page);
    await page.keyboard.press(`${MOD}+Shift+P`);
    const option = palette(page).getByRole("option", { name: /Compact conversation/ });
    await expect(option).toContainText("Ctrl+Shift+K");
    await expect(palette(page).getByRole("option", { name: /New chat/ })).toContainText("Ctrl+N");
  });

  test("explains why a command is unavailable instead of hiding it", async ({ page }) => {
    await openChat(page);
    await page.keyboard.press(`${MOD}+Shift+P`);
    const approve = palette(page).getByRole("option", { name: /Allow this tool call once/ });
    await expect(approve).toHaveAttribute("aria-disabled", "true");
    await expect(approve).toContainText("No tool is waiting for approval.");
    // A disabled row can still be chosen: it says why, rather than doing nothing.
    // Playwright treats aria-disabled as unclickable, so the click is forced.
    await approve.click({ force: true });
    await expect(page.getByRole("status").getByText("No tool is waiting for approval.")).toBeVisible();
    await expect(palette(page)).toBeVisible();
  });

  test("switches the model from the keyboard", async ({ page }) => {
    await openChat(page);
    await page.keyboard.press(`${MOD}+Shift+M`);
    await expect(palette(page).getByRole("combobox")).toHaveAttribute("placeholder", "Choose a model…");
    const options = palette(page).getByRole("option");
    await expect(options.first()).toBeVisible();
    await palette(page).getByRole("combobox").press("Enter");
    await expect(palette(page)).toHaveCount(0);
    await expect(page.getByText(/Model set to/)).toBeVisible();
  });

  test("switches the workspace from the keyboard", async ({ page }) => {
    await openChat(page);
    await page.keyboard.press(`${MOD}+Shift+O`);
    await expect(palette(page).getByRole("combobox")).toHaveAttribute("placeholder", "Choose a workspace…");
    await palette(page).getByRole("option", { name: /Browse for a folder/ }).click();
    await expect(page.getByText("Workspace updated for this chat.")).toBeVisible();
  });

  test("Escape steps back from a sub-page, then closes", async ({ page }) => {
    await openChat(page);
    await page.keyboard.press(`${MOD}+Shift+M`);
    const search = palette(page).getByRole("combobox");
    await search.press("Escape");
    await expect(search).toHaveAttribute("placeholder", "Type a command…");
    await search.press("Escape");
    await expect(palette(page)).toHaveCount(0);
  });

  test("starts compaction, with its confirmation, from the keyboard", async ({ page }) => {
    await openChat(page);
    await page.keyboard.press(`${MOD}+Shift+K`);
    await expect(page.getByRole("dialog", { name: /Compact this conversation/ })).toBeVisible();
  });
});

test.describe("search", () => {
  test("Ctrl+K still searches chats", async ({ page }) => {
    await openChat(page);
    await page.keyboard.press(`${MOD}+k`);
    await expect(page.getByPlaceholder("Search conversations…")).toBeVisible();
  });

  test("typing > turns the search into the command palette", async ({ page }) => {
    await openChat(page);
    await page.keyboard.press(`${MOD}+k`);
    await page.getByPlaceholder("Search conversations…").fill(">comp");
    await expect(palette(page)).toBeVisible();
    await expect(palette(page).getByRole("combobox")).toHaveValue("comp");
    await expect(palette(page).getByRole("option", { name: /Compact conversation/ })).toBeVisible();
    await expect(page.getByPlaceholder("Search conversations…")).toHaveCount(0);
  });

  test("Escape in the search does not stop a running turn", async ({ page }) => {
    await openChat(page);
    const composer = page.getByRole("textbox", { name: "Message" });
    await composer.fill("a quick message");
    await composer.press("Enter");
    await expect(page.getByRole("button", { name: "Stop generating" })).toBeVisible();
    await page.keyboard.press(`${MOD}+k`);
    await page.getByPlaceholder("Search conversations…").press("Escape");
    // The search closed, and the turn kept going until it finished by itself.
    await expect(page.getByPlaceholder("Search conversations…")).toHaveCount(0);
    await expect(page.getByText(/I received/)).toBeVisible({ timeout: 15_000 });
    await expect(page.getByText("Stopped.")).toHaveCount(0);
  });
});

test.describe("stopping a turn", () => {
  test("Escape in the message box stops it", async ({ page }) => {
    await openChat(page);
    const composer = page.getByRole("textbox", { name: "Message" });
    await composer.fill("stop me");
    await composer.press("Enter");
    await expect(page.getByRole("button", { name: "Stop generating" })).toBeVisible();
    // Keep the shortcut scoped to the message box, even if a render moved focus.
    await composer.focus();
    await expect(composer).toBeFocused();
    await page.keyboard.press("Escape");
    await expect(page.getByRole("button", { name: "Send message" })).toBeVisible({ timeout: 10_000 });
  });

  test("Escape when idle does nothing and shows nothing", async ({ page }) => {
    await openChat(page);
    await page.getByRole("textbox", { name: "Message" }).press("Escape");
    await expect(page.getByRole("status")).toHaveCount(0);
  });
});

test.describe("approvals", () => {
  test("shows the rule that Always allow will save", async ({ page }) => {
    await openChat(page);
    const { dialog } = await askForApproval(page);
    const rule = dialog.getByTestId("approval-rule");
    await expect(rule).toContainText("Always allow will save");
    await expect(rule).toContainText("builtin.bash");
    await expect(dialog.getByTestId("approval-pattern")).toHaveText("npm test");
    await expect(rule).toContainText("Exact rule");
    await dialog.getByRole("button", { name: "Always allow" }).click();
    await expect(dialog).toHaveCount(0);
  });

  test("Ctrl+Enter allows the call once", async ({ page }) => {
    await openChat(page);
    const { dialog } = await askForApproval(page);
    await dialog.focus();
    await page.keyboard.press(`${MOD}+Enter`);
    await expect(dialog).toHaveCount(0);
    const row = page.getByRole("button", { name: /Bash/ });
    await expect(row).toContainText("exit 0");
  });

  test("Ctrl+Backspace rejects the call", async ({ page }) => {
    await openChat(page);
    const { dialog } = await askForApproval(page);
    await dialog.focus();
    await page.keyboard.press(`${MOD}+Backspace`);
    await expect(dialog).toHaveCount(0);
    await expect(page.getByRole("button", { name: /Bash/ })).toContainText("The user declined this tool call");
  });

  test("never answers from a text field, so a rejection reason can be typed", async ({ page }) => {
    await openChat(page);
    const { dialog } = await askForApproval(page);
    await dialog.getByRole("button", { name: "Reject", exact: true }).click();
    const reason = dialog.getByPlaceholder(/use the read-only search tool/);
    await reason.fill("please use read instead");
    // Both chords are ordinary editing here; neither may answer the call.
    await reason.press(`${MOD}+Backspace`);
    await reason.press(`${MOD}+Enter`);
    await expect(dialog).toBeVisible();
    await expect(page.getByRole("button", { name: /Bash/ })).toContainText("running");
  });

  test("the approval chords do nothing when no call is waiting", async ({ page }) => {
    await openChat(page);
    const composer = page.getByRole("textbox", { name: "Message" });
    await composer.fill("two words");
    await composer.press(`${MOD}+Backspace`);
    // Ctrl+Backspace still deletes a word in the message box.
    await expect(composer).toHaveValue("two ");
    await expect(page.getByRole("dialog")).toHaveCount(0);
  });
});
