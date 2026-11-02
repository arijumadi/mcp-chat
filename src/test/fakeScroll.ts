/**
 * jsdom has no layout. This models a scroll container with a 100px viewport
 * where every character of text content is 1px tall, and clamps `scrollTop`
 * the way a browser does. Call `installFakeScroll()` in `beforeEach` and
 * `removeFakeScroll()` in `afterEach`.
 */
export const FAKE_VIEWPORT = 100;

const scrollTops = new WeakMap<Element, number>();
const maxTop = (element: Element) => Math.max(0, (element.textContent?.length ?? 0) - FAKE_VIEWPORT);

export function installFakeScroll() {
  Object.defineProperty(HTMLElement.prototype, "scrollHeight", {
    configurable: true,
    get(this: HTMLElement) {
      return this.textContent?.length ?? 0;
    },
  });
  Object.defineProperty(HTMLElement.prototype, "clientHeight", {
    configurable: true,
    get() {
      return FAKE_VIEWPORT;
    },
  });
  Object.defineProperty(HTMLElement.prototype, "scrollTop", {
    configurable: true,
    get(this: HTMLElement) {
      return Math.min(scrollTops.get(this) ?? 0, maxTop(this));
    },
    set(this: HTMLElement, value: number) {
      scrollTops.set(this, Math.max(0, Math.min(value, maxTop(this))));
    },
  });
}

export function removeFakeScroll() {
  const proto = HTMLElement.prototype as unknown as Record<string, unknown>;
  delete proto.scrollHeight;
  delete proto.clientHeight;
  delete proto.scrollTop;
}
