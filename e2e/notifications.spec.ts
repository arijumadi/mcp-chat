import { expect, test, type Page } from "@playwright/test";

interface Shown {
  title: string;
  body: string;
}

/**
 * Replace the Web Notification API with a recorder, and make the page report
 * whether the window is in front. `window.__away` flips that.
 */
async function install(page: Page, permission: NotificationPermission = "granted") {
  await page.addInitScript((initial) => {
    const shown: Array<{ title: string; body: string }> = [];
    (window as unknown as { __shown: typeof shown }).__shown = shown;
    (window as unknown as { __away: boolean }).__away = false;
    class FakeNotification {
      static permission: NotificationPermission = initial;
      static async requestPermission() {
        return FakeNotification.permission;
      }
      constructor(title: string, options?: { body?: string }) {
        shown.push({ title, body: options?.body ?? "" });
      }
    }
    (window as unknown as { Notification: unknown }).Notification = FakeNotification;
    document.hasFocus = () => !(window as unknown as { __away: boolean }).__away;
  }, permission);
}

const shown = (page: Page) =>
  page.evaluate(() => (window as unknown as { __shown: Shown[] }).__shown);

const goAway = (page: Page) =>
  page.evaluate(() => {
    (window as unknown as { __away: boolean }).__away = true;
  });

async function openDemo(page: Page) {
  await page.goto("/?mock=1");
  await page.getByText("Welcome to Jarvis").first().click();
  await expect(page.getByText("What can you do with MCP tools?")).toBeVisible();
}

test("a finished reply is announced when the window is in the background", async ({ page }) => {
  await install(page);
  await openDemo(page);
  const composer = page.getByRole("textbox", { name: "Message" });
  await composer.fill("hello");
  await composer.press("Enter");
  await goAway(page);
  await expect(page.getByText(/I received/)).toBeVisible({ timeout: 15_000 });
  await expect.poll(async () => (await shown(page)).length).toBe(1);
  expect((await shown(page))[0]).toEqual({ title: "Jarvis · Welcome to Jarvis", body: "Finished." });
});

test("nothing is announced while the user is looking at the chat", async ({ page }) => {
  await install(page);
  await openDemo(page);
  const composer = page.getByRole("textbox", { name: "Message" });
  await composer.fill("hello");
  await composer.press("Enter");
  await expect(page.getByText(/I received/)).toBeVisible({ timeout: 15_000 });
  await expect(page.getByRole("button", { name: "Send message" })).toBeVisible();
  expect(await shown(page)).toHaveLength(0);
});

test("a tool waiting for approval is announced with its name only", async ({ page }) => {
  await install(page);
  await openDemo(page);
  const composer = page.getByRole("textbox", { name: "Message" });
  await composer.fill("this needs approval");
  // Leave before the short scripted approval delay; the event can otherwise
  // land before the page call that flips visibility.
  await goAway(page);
  await composer.press("Enter");
  await expect(page.getByRole("dialog", { name: /Tool permission/ })).toBeVisible();
  await expect.poll(async () => (await shown(page)).length).toBe(1);
  const [notice] = await shown(page);
  expect(notice!.body).toBe("Waiting for your approval: bash");
  // The command the model wants to run must not leak into the notification.
  expect(JSON.stringify(notice)).not.toContain("npm test");
});

test("stopping a turn yourself does not notify", async ({ page }) => {
  await install(page);
  await openDemo(page);
  const composer = page.getByRole("textbox", { name: "Message" });
  await composer.fill("stop me");
  await composer.press("Enter");
  await expect(page.getByRole("button", { name: "Stop generating" })).toBeVisible();
  await goAway(page);
  await page.evaluate(() => (document.activeElement as HTMLElement | null)?.blur());
  await page.getByRole("button", { name: "Stop generating" }).click();
  await expect(page.getByRole("button", { name: "Send message" })).toBeVisible({ timeout: 10_000 });
  expect(await shown(page)).toHaveLength(0);
});

test("nothing is sent when the setting is off", async ({ page }) => {
  await install(page);
  await openDemo(page);
  await page.getByRole("complementary").getByRole("button", { name: "Settings" }).click();
  await page.getByRole("checkbox", { name: /Notify me when I am away/ }).uncheck();
  await page.getByText("Welcome to Jarvis").first().click();
  const composer = page.getByRole("textbox", { name: "Message" });
  await composer.fill("hello");
  await composer.press("Enter");
  await goAway(page);
  await expect(page.getByText(/I received/)).toBeVisible({ timeout: 15_000 });
  await page.waitForTimeout(400);
  expect(await shown(page)).toHaveLength(0);
});

test("nothing is sent when the system denied permission, and nothing is asked in the background", async ({
  page,
}) => {
  await install(page, "default");
  await page.addInitScript(() => {
    (window as unknown as { __asked: number }).__asked = 0;
    const fake = (window as unknown as { Notification: { requestPermission: () => Promise<string> } }).Notification;
    fake.requestPermission = async () => {
      (window as unknown as { __asked: number }).__asked += 1;
      return "default";
    };
  });
  await openDemo(page);
  const composer = page.getByRole("textbox", { name: "Message" });
  await composer.fill("hello");
  await composer.press("Enter");
  await goAway(page);
  await expect(page.getByText(/I received/)).toBeVisible({ timeout: 15_000 });
  await page.waitForTimeout(300);
  expect(await shown(page)).toHaveLength(0);
  // The only request happened when the message was sent, with the app in front.
  expect(await page.evaluate(() => (window as unknown as { __asked: number }).__asked)).toBe(1);
});

test("the test button sends a notification", async ({ page }) => {
  await install(page);
  await openDemo(page);
  await page.getByRole("complementary").getByRole("button", { name: "Settings" }).click();
  await page.getByRole("button", { name: "Send a test notification" }).click();
  await expect(page.getByText("Test notification sent.")).toBeVisible();
  expect(await shown(page)).toEqual([{ title: "Jarvis", body: "Notifications are on." }]);
});

test("the test button explains a blocked system permission", async ({ page }) => {
  await install(page, "denied");
  await openDemo(page);
  await page.getByRole("complementary").getByRole("button", { name: "Settings" }).click();
  await page.getByRole("button", { name: "Send a test notification" }).click();
  await expect(page.getByText(/blocked/i)).toBeVisible();
  expect(await shown(page)).toHaveLength(0);
});
