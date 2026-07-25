import type { Locator } from "@playwright/test";

// `<input type="range">` isn't drag-simulable via `.fill()`/`.type()` — set
// the DOM value directly and dispatch the native `input` event, the same
// event a real drag emits and the one Dioxus's `oninput` handler listens
// for. Shared by the full player's volume slider and the mini-dock's
// compact one so both flows exercise the control the same way.
export async function setRangeValue(
  slider: Locator,
  value: number,
): Promise<void> {
  await slider.evaluate((el, v) => {
    const input = el as HTMLInputElement;
    input.value = String(v);
    input.dispatchEvent(new Event("input", { bubbles: true }));
  }, value);
}
