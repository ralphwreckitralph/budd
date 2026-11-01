// The card page: a compose box and a Close button. It never takes the keyboard on its own; Rust
// says when, with `focus_compose`, after making the window key on a human's intent.
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { debug } from "@tauri-apps/plugin-log";

const compose = document.getElementById("compose") as HTMLTextAreaElement;
const close = document.getElementById("close") as HTMLButtonElement;

function hide() {
  // Dev builds log at debug level: the draft is the evidence that typed keys landed here.
  void debug(`card hides with draft ${JSON.stringify(compose.value)}`);
  void invoke("card_hide");
}

close.addEventListener("click", hide);

// Esc anywhere on the card closes it, not only in the compose box.
document.addEventListener("keydown", (event) => {
  void debug(`card keydown ${event.key} on ${document.activeElement?.tagName ?? "nothing"}, focus=${document.hasFocus()}`);
  if (event.key === "Escape") {
    event.preventDefault();
    hide();
  }
});

void listen("focus_compose", () => {
  compose.focus();
  // A kept draft is added to, not typed over.
  compose.setSelectionRange(compose.value.length, compose.value.length);
  void debug(`card focused compose: active=${document.activeElement?.tagName}, focus=${document.hasFocus()}`);
});
