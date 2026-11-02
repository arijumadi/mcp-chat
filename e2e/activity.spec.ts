import { expect, test, type Page } from "@playwright/test";

async function openDemo(page: Page) {
  await page.goto("/?mock=1");
  await page.getByText("Welcome to Jarvis").first().click();
  await expect(page.getByText("What can you do with MCP tools?")).toBeVisible();
}

const thoughtRow = (page: Page) => page.getByRole("button", { name: /^Thought/ });
const editRow = (page: Page) => page.getByRole("button", { name: /Edit\s+src\/app\.ts/ });

test("rows are compact and collapsed by default", async ({ page }) => {
  await openDemo(page);

  // Every thought and tool is visible as one row, without opening a group.
  await expect(thoughtRow(page)).toBeVisible();
  await expect(page.getByRole("button", { name: /Mock MCP\/echo/ })).toBeVisible();
  await expect(editRow(page)).toBeVisible();
  await expect(editRow(page)).toContainText("+2");
  await expect(editRow(page)).toContainText("\u22121");

  // Nothing inside them is rendered yet: no thought text, arguments, results or diff.
  await expect(page.getByText("The user wants to know")).toHaveCount(0);
  await expect(page.getByText("Arguments", { exact: true })).toHaveCount(0);
  await expect(page.locator("pre", { hasText: "echo: hello from MCP" })).toHaveCount(0);
  await expect(page.locator(".diff-line")).toHaveCount(0);

  // A row is one line tall.
  const box = await editRow(page).boundingBox();
  expect(box!.height).toBeLessThan(40);
});

test("a thought shows only its label until opened", async ({ page }) => {
  await openDemo(page);
  const row = thoughtRow(page);
  await expect(row).toHaveText("Thought");
  await row.click();
  await expect(page.getByText("The user wants to know what MCP tools can do.")).toBeVisible();
  await row.click();
  await expect(page.getByText("The user wants to know")).toHaveCount(0);
});

test("an edit keeps its diff collapsed inside the tool and colors it once opened", async ({
  page,
}) => {
  await openDemo(page);
  await editRow(page).click();
  await expect(page.getByText("Arguments", { exact: true })).toBeVisible();
  await expect(page.getByText("Result", { exact: true })).toBeVisible();

  const changes = page.getByRole("button", { name: /Changes/ });
  await expect(changes).toHaveAttribute("aria-expanded", "false");
  await expect(changes).toContainText("+2");
  await expect(page.locator(".diff-line")).toHaveCount(0);

  await changes.click();
  await expect(page.locator(".diff-line-added")).toHaveCount(2);
  await expect(page.locator(".diff-line-removed")).toHaveCount(1);
  await expect(page.locator(".diff-hunk")).toHaveCount(1);
  await expect(page.locator(".diff-file-header")).toHaveCount(2);

  const colors = await page.evaluate(() => {
    const bg = (selector: string) =>
      getComputedStyle(document.querySelector(selector)!).backgroundColor;
    return { added: bg(".diff-line-added"), removed: bg(".diff-line-removed") };
  });
  expect(colors.added).not.toBe("rgba(0, 0, 0, 0)");
  expect(colors.removed).not.toBe("rgba(0, 0, 0, 0)");
  expect(colors.added).not.toBe(colors.removed);
});

test("diff colors are present in light and dark themes", async ({ page }) => {
  await openDemo(page);
  await editRow(page).click();
  await page.getByRole("button", { name: /Changes/ }).click();

  const read = () =>
    page.evaluate(() => {
      const style = (selector: string) => {
        const element = document.querySelector(selector)!;
        const computed = getComputedStyle(element);
        return { bg: computed.backgroundColor, color: computed.color };
      };
      return {
        added: style(".diff-line-added"),
        removed: style(".diff-line-removed"),
        body: getComputedStyle(document.querySelector(".diff-body")!).backgroundColor,
      };
    });

  const light = await read();
  await page.evaluate(() => {
    document.documentElement.dataset.theme = "dark";
  });
  const dark = await read();
  for (const theme of [light, dark]) {
    expect(theme.added.bg).not.toBe("rgba(0, 0, 0, 0)");
    expect(theme.removed.bg).not.toBe("rgba(0, 0, 0, 0)");
    expect(theme.added.bg).not.toBe(theme.removed.bg);
    expect(theme.added.color).not.toBe(theme.added.bg);
  }
  expect(dark.body).not.toBe(light.body);
});

test("rows return to collapsed after a reload, and the diff can be reopened", async ({ page }) => {
  await openDemo(page);
  await thoughtRow(page).click();
  await editRow(page).click();
  await page.getByRole("button", { name: /Changes/ }).click();
  await expect(page.locator(".diff-line-added")).toHaveCount(2);

  await page.reload();
  await page.getByText("Welcome to Jarvis").first().click();
  await expect(page.getByText("What can you do with MCP tools?")).toBeVisible();
  await expect(thoughtRow(page)).toHaveAttribute("aria-expanded", "false");
  await expect(editRow(page)).toHaveAttribute("aria-expanded", "false");
  await expect(page.locator(".diff-line")).toHaveCount(0);

  // The diff was stored with the tool call, so it is still there.
  await editRow(page).click();
  await page.getByRole("button", { name: /Changes/ }).click();
  await expect(page.locator(".diff-line-removed")).toHaveCount(1);
});

test("rows return to collapsed after switching chats", async ({ page }) => {
  await openDemo(page);
  await editRow(page).click();
  await expect(editRow(page)).toHaveAttribute("aria-expanded", "true");

  await page.getByRole("button", { name: "New chat" }).first().click();
  await expect(editRow(page)).toHaveCount(0);
  await page.getByText("Welcome to Jarvis").first().click();
  await expect(page.getByText("What can you do with MCP tools?")).toBeVisible();
  await expect(editRow(page)).toHaveAttribute("aria-expanded", "false");
  await expect(page.getByText("Arguments", { exact: true })).toHaveCount(0);
});

test.describe("minimum window width", () => {
  test.use({ viewport: { width: 860, height: 860 } });

  test("expanded rows and a long diff stay inside the viewport", async ({ page }) => {
    await openDemo(page);
    await thoughtRow(page).click();
    await page.getByRole("button", { name: /Mock MCP\/echo/ }).click();
    await editRow(page).click();
    await page.getByRole("button", { name: /Changes/ }).click();
    await expect(page.locator(".diff-line-added")).toHaveCount(2);

    const overflows = await page.evaluate(
      () => document.documentElement.scrollWidth > document.documentElement.clientWidth,
    );
    expect(overflows).toBe(false);
    const row = await editRow(page).boundingBox();
    expect(row!.height).toBeLessThan(40);
  });
});
