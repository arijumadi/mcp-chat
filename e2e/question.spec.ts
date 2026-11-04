import { expect, test, type Page } from "@playwright/test";

async function openChat(page: Page) {
  await page.goto("/?mock=1");
  await page.getByText("Welcome to Jarvis").first().click();
  await expect(page.getByText("What can you do with MCP tools?")).toBeVisible();
}

/** Starts the scripted turn in which the model asks one question without options. */
async function askFreeFormQuestion(page: Page) {
  const composer = page.getByRole("textbox", { name: "Message" });
  await composer.fill("this needs an answer");
  await composer.press("Enter");
  const dialog = page.getByRole("dialog", { name: "The model needs input" });
  await expect(dialog).toBeVisible();
  return dialog;
}

test.describe("question tool", () => {
  test("a free-form question is answered by typing, and the turn carries on", async ({ page }) => {
    await openChat(page);
    const dialog = await askFreeFormQuestion(page);

    await expect(dialog.getByText("Which database should I target?")).toBeVisible();
    // No options: no choices to pick, just a field with the cursor already in it.
    await expect(dialog.getByRole("radio")).toHaveCount(0);
    await expect(dialog.getByRole("checkbox")).toHaveCount(0);
    const answer = dialog.getByRole("textbox", { name: "Answer for Database" });
    await expect(answer).toHaveAttribute("placeholder", "Type your answer…");
    await expect(answer).toBeFocused();

    // An empty answer is refused and the question stays open.
    await dialog.getByRole("button", { name: "Send answer" }).click();
    await expect(dialog.getByText('Answer "Database" to continue.')).toBeVisible();

    await answer.fill("Postgres 16");
    await answer.press("Enter");

    await expect(dialog).toHaveCount(0);
    await expect(page.getByText("Thanks. I will target Postgres 16.")).toBeVisible();
    // The turn is over: the composer offers Send again, not Stop.
    await expect(page.getByRole("button", { name: "Send message" })).toBeVisible();
  });

  test("the question shows up as an ordinary activity row with its result", async ({ page }) => {
    await openChat(page);
    const dialog = await askFreeFormQuestion(page);
    await dialog.getByRole("textbox", { name: "Answer for Database" }).fill("SQLite");
    await dialog.getByRole("button", { name: "Send answer" }).click();
    await expect(dialog).toHaveCount(0);

    await expect(page.getByText("Thanks. I will target SQLite.")).toBeVisible();

    // The question is one line in the transcript, like any other tool call.
    const row = page.locator('[data-tool-name="question"]');
    await expect(row).toHaveCount(1);
    await expect(row).toHaveAttribute("data-tool-state", "completed");
    await row.getByRole("button", { name: /Question/ }).click();
    await expect(row.getByText("The user answered: Database: SQLite")).toBeVisible();
  });

  test("declining a question ends the tool call without an answer", async ({ page }) => {
    await openChat(page);
    const dialog = await askFreeFormQuestion(page);

    await dialog.getByRole("button", { name: "Decline" }).click();

    await expect(dialog).toHaveCount(0);
    await expect(page.getByText(/Thanks\. I will target/)).toHaveCount(0);
    await expect(page.getByRole("button", { name: "Send message" })).toBeVisible();
  });
});
