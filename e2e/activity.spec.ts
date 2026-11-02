import { expect, test, type Locator, type Page } from "@playwright/test";

async function openDemo(page: Page) {
  await page.goto("/?mock=1");
  await page.getByText("Welcome to Jarvis").first().click();
  await expect(page.getByText("What can you do with MCP tools?")).toBeVisible();
}

const thoughtRow = (page: Page) => page.getByRole("button", { name: /^Thought/ });
const tool = (page: Page, name: string) => page.locator(`[data-tool-name="${name}"]`);
const toolRow = (page: Page, name: string) => tool(page, name).getByRole("button").first();
const editRow = (page: Page) => page.getByRole("button", { name: /Edit\s+src\/app\.ts/ });

test("rows are compact and collapsed by default", async ({ page }) => {
  await openDemo(page);

  // Every thought and tool is visible as one row, without opening a group.
  await expect(thoughtRow(page)).toBeVisible();
  await expect(page.getByRole("button", { name: /Mock MCP\/echo/ })).toBeVisible();
  await expect(editRow(page)).toBeVisible();
  await expect(editRow(page)).toContainText("+2");
  await expect(editRow(page)).toContainText("\u22121");
  await expect(toolRow(page, "write")).toContainText("docs/notes.md");
  await expect(toolRow(page, "apply_patch")).toContainText("2 files");

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

for (const name of ["edit", "write", "apply_patch"] as const) {
  test(`${name} opens its diff with a single click`, async ({ page }) => {
    await openDemo(page);
    const card = tool(page, name);
    await toolRow(page, name).click();

    // The diff is already open: no Changes toggle to click, no Arguments or Result.
    const changes = card.getByRole("button", { name: /Changes/ });
    await expect(changes).toHaveAttribute("aria-expanded", "true");
    await expect(card.locator(".diff-line-added").first()).toBeVisible();
    await expect(card.getByText("Arguments", { exact: true })).toHaveCount(0);
    await expect(card.getByText("Result", { exact: true })).toHaveCount(0);
    await expect(card.locator("pre")).toHaveCount(0);
    // Only the row itself and the diff summary are buttons.
    await expect(card.getByRole("button")).toHaveCount(2);
  });
}

test("an edit's diff colors added and removed lines", async ({ page }) => {
  await openDemo(page);
  await editRow(page).click();
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

test("a multi-file patch shows every file in one body", async ({ page }) => {
  await openDemo(page);
  await toolRow(page, "apply_patch").click();
  const card = tool(page, "apply_patch");
  await expect(card.locator(".diff-file-header")).toHaveCount(4);
  await expect(card.getByText("--- a/src/util.ts")).toBeVisible();
  await expect(card.getByText("--- a/src/index.ts")).toBeVisible();
  await expect(card.getByRole("button", { name: /Changes/ })).toContainText("2 files");
});

test("regular tools keep their arguments and result view", async ({ page }) => {
  await openDemo(page);
  // The mock tool is a regular MCP tool. bash and read are covered in ToolCard unit tests.
  await page.getByRole("button", { name: /Mock MCP\/echo/ }).click();
  await expect(page.getByText("Arguments", { exact: true })).toBeVisible();
  await expect(page.getByText("Result", { exact: true })).toBeVisible();
  await expect(page.locator(".diff-view")).toHaveCount(0);
});

test("diff colors are present in light and dark themes", async ({ page }) => {
  await openDemo(page);
  await editRow(page).click();
  await expect(page.locator(".diff-line-added").first()).toBeVisible();

  const read = () =>
    page.evaluate(() => {
      const style = (selector: string) => {
        const computed = getComputedStyle(document.querySelector(selector)!);
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

test("rows return to collapsed after a reload, and the diff opens again in one click", async ({
  page,
}) => {
  await openDemo(page);
  await thoughtRow(page).click();
  await editRow(page).click();
  await expect(page.locator(".diff-line-added")).toHaveCount(2);

  await page.reload();
  await page.getByText("Welcome to Jarvis").first().click();
  await expect(page.getByText("What can you do with MCP tools?")).toBeVisible();
  await expect(thoughtRow(page)).toHaveAttribute("aria-expanded", "false");
  await expect(editRow(page)).toHaveAttribute("aria-expanded", "false");
  await expect(page.locator(".diff-line")).toHaveCount(0);

  // The diff was stored with the tool call, so one click shows it again.
  await editRow(page).click();
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
  await expect(page.locator(".diff-line")).toHaveCount(0);
});

async function startLongThought(page: Page) {
  await page.goto("/?mock=1");
  await page.getByRole("button", { name: "New chat" }).first().click();
  const composer = page.getByRole("textbox", { name: "Message" });
  await composer.fill("long thought please");
  await composer.press("Enter");
  const row = thoughtRow(page);
  await expect(row).toBeVisible();
  await expect(row).toHaveText("Thought");
  await row.click();
  const body = page.getByRole("region", { name: "Thought text" });
  await expect(body).toBeVisible();
  return body;
}

const metrics = (body: Locator) =>
  body.evaluate((element) => ({
    top: element.scrollTop,
    height: element.scrollHeight,
    gap: element.scrollHeight - element.scrollTop - element.clientHeight,
  }));

test.describe("streaming thought", () => {
  test("follows new text, stops when the user scrolls up, and resumes at the bottom", async ({
    page,
  }) => {
    const body = await startLongThought(page);

    // 1. Taller than its viewport, and pinned to the newest line.
    await expect.poll(async () => (await metrics(body)).height).toBeGreaterThan(450);
    await expect.poll(async () => (await metrics(body)).gap).toBeLessThan(24);
    const pinned = (await metrics(body)).height;
    await expect.poll(async () => (await metrics(body)).height).toBeGreaterThan(pinned + 60);
    expect((await metrics(body)).gap).toBeLessThan(24);

    // 2. Scroll to the top. More text arrives, but the view stays where the user put it.
    await body.evaluate((element) => {
      element.scrollTop = 0;
    });
    const atTop = (await metrics(body)).height;
    await expect.poll(async () => (await metrics(body)).height).toBeGreaterThan(atTop + 100);
    expect((await metrics(body)).top).toBeLessThan(5);

    // 3. Back to the bottom: it follows again as the thought keeps growing.
    await body.evaluate((element) => {
      element.scrollTop = element.scrollHeight;
    });
    const resumed = (await metrics(body)).height;
    await expect.poll(async () => (await metrics(body)).height).toBeGreaterThan(resumed + 60);
    expect((await metrics(body)).gap).toBeLessThan(24);
  });

  test("only the thought body scrolls, not the page", async ({ page }) => {
    const body = await startLongThought(page);
    await expect.poll(async () => (await metrics(body)).height).toBeGreaterThan(450);
    const overflows = await page.evaluate(
      () => document.documentElement.scrollWidth > document.documentElement.clientWidth,
    );
    expect(overflows).toBe(false);
    expect(await page.evaluate(() => document.documentElement.scrollTop)).toBe(0);
  });

  test("starts at the bottom again after collapsing and reopening", async ({ page }) => {
    const body = await startLongThought(page);
    await expect.poll(async () => (await metrics(body)).height).toBeGreaterThan(450);
    await body.evaluate((element) => {
      element.scrollTop = 0;
    });
    await thoughtRow(page).click();
    await expect(body).toHaveCount(0);
    await thoughtRow(page).click();
    const reopened = page.getByRole("region", { name: "Thought text" });
    await expect.poll(async () => (await metrics(reopened)).gap).toBeLessThan(24);
    expect((await metrics(reopened)).top).toBeGreaterThan(100);
  });
});

test.describe("minimum window width", () => {
  test.use({ viewport: { width: 860, height: 860 } });

  test("expanded rows and a long diff stay inside the viewport", async ({ page }) => {
    await openDemo(page);
    await thoughtRow(page).click();
    await page.getByRole("button", { name: /Mock MCP\/echo/ }).click();
    await editRow(page).click();
    await toolRow(page, "write").click();
    await toolRow(page, "apply_patch").click();
    await expect(page.locator(".diff-line-added").first()).toBeVisible();

    const overflows = await page.evaluate(
      () => document.documentElement.scrollWidth > document.documentElement.clientWidth,
    );
    expect(overflows).toBe(false);
    const row = await editRow(page).boundingBox();
    expect(row!.height).toBeLessThan(40);
  });
});
