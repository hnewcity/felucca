import { expect, it, vi } from "vitest";
import { loadPage } from "./load-page.js";

it("re-reads a page if its overview and configuration belong to different revisions", async () => {
  const api = vi
    .fn()
    .mockResolvedValueOnce({ revision: 1 })
    .mockResolvedValueOnce({ revision: 2, config: { changed: true } })
    .mockResolvedValueOnce({ revision: 2 })
    .mockResolvedValueOnce({ revision: 2, config: { changed: true } });
  const data = await loadPage(api, "config", "", 0);
  expect(data.revision).toBe(2);
  expect(data.config).toEqual({ changed: true });
  expect(api).toHaveBeenCalledTimes(4);
});

it("issues independent reads of a page together instead of one after another", async () => {
  const pending = [];
  const api = vi.fn(
    (path) =>
      new Promise((resolve) =>
        pending.push(() =>
          resolve(
            path === "/api/clients"
              ? { revision: 1, config: [] }
              : path === "/api/agent-packages"
                ? { revision: 1, packages: [] }
                : { revision: 1, config: [] },
          ),
        ),
      ),
  );
  const loading = loadPage(api, "tunnels", "", 0);
  await Promise.resolve();
  expect(api.mock.calls.map(([path]) => path).sort()).toEqual([
    "/api/agent-packages",
    "/api/clients",
    "/api/launch",
    "/api/overview",
    "/api/shares",
  ]);
  pending.forEach((resolve) => resolve());
  const data = await loading;
  expect(data.revision).toBe(1);
  expect(data.managedByClient).toEqual({});
});
