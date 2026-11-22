import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

export const ipc = {
  language: () => invoke("language"),
  list: () => invoke("list"),
  describe: (text) => invoke("describe", { text }),
  add: (text, name) => invoke("add", { text, name }),
  remove: (id) => invoke("remove", { id }),
  setEnabled: (id, enabled) => invoke("set_enabled", { id, enabled }),
  takeLinks: () => invoke("take_links"),
  autostart: () => invoke("autostart"),
  setAutostart: (enabled) => invoke("set_autostart", { enabled }),
  onLinks: (callback) => listen("links", callback),
};
