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

// Commit a range value the way releasing a drag does: `input` (live preview)
// followed by `change` (the release). The listen-page scrubber previews on
// `input` but only seeks the audio on `change`, so a test that wants the seek
// to actually fire must dispatch both. Mirrors a real pointer-up.
export async function commitRangeValue(
  slider: Locator,
  value: number,
): Promise<void> {
  await slider.evaluate((el, v) => {
    const input = el as HTMLInputElement;
    input.value = String(v);
    input.dispatchEvent(new Event("input", { bubbles: true }));
    input.dispatchEvent(new Event("change", { bubbles: true }));
  }, value);
}
