import { describe, expect, it, vi } from "vitest";
import { translate } from "./i18n.js";
import { createApi } from "./api.js";

const response = (value = {}, status = 200) => new Response(JSON.stringify(value), { status });
const setup = (fetcher) => {
  const onUnauthorized = vi.fn();
  return {
    api: createApi({
      getSession: () => ({ csrf: "csrf-test" }),
      onUnauthorized,
      getLocale: () => "en",
      translate: (key, args) => translate("en", key, args),
      fetcher,
    }),
    onUnauthorized,
  };
};
describe("management request contract", () => {
  it("sends the editor's revision, CSRF token and same-origin credentials", async () => {
    const fetcher = vi.fn().mockResolvedValue(response({ revision: 8 }));
    const { api } = setup(fetcher);
    await api("/api/clients/one", "PUT", { remark: "edited" }, { revision: 7 });
    const options = fetcher.mock.calls[0][1];
    expect(options.headers["If-Match"]).toBe('"7"');
    expect(options.headers["X-Felucca-CSRF"]).toBe("csrf-test");
    expect(options.headers["Idempotency-Key"]).toBeTruthy();
    expect(options.credentials).toBe("same-origin");
  });
  it("retains idempotency keys across uncertain network failures", async () => {
    const fetcher = vi
      .fn()
      .mockRejectedValueOnce(new TypeError("connection lost"))
      .mockResolvedValue(response());
    const { api } = setup(fetcher);
    await expect(api("/api/managed/one", "PUT", { tunnel: [] }, { revision: 4 })).rejects.toThrow(
      "connection lost",
    );
    await api("/api/managed/one", "PUT", { tunnel: [] }, { revision: 4 });
    expect(fetcher.mock.calls[0][1].headers["Idempotency-Key"]).toBe(
      fetcher.mock.calls[1][1].headers["Idempotency-Key"],
    );
  });
  it("does not retry a conflict with an implicitly newer revision", async () => {
    const fetcher = vi.fn().mockResolvedValue(response({ error: "stale revision" }, 409));
    const { api } = setup(fetcher);
    await expect(api("/api/config", "PUT", {}, { revision: 2 })).rejects.toMatchObject({
      status: 409,
    });
    expect(fetcher).toHaveBeenCalledTimes(1);
  });
  it("does not attach configuration preconditions to CPU sampling or validation", async () => {
    const fetcher = vi.fn().mockImplementation(() => Promise.resolve(response()));
    const { api } = setup(fetcher);
    await api("/api/profile", "POST", { seconds: 5 }, { raw: true });
    await api("/api/validate", "POST", {});
    for (const [, options] of fetcher.mock.calls)
      expect(options.headers["If-Match"]).toBeUndefined();
  });
  it("revokes UI sessions on 401, without treating bad login credentials as a session", async () => {
    const fetcher = vi.fn().mockImplementation(() => Promise.resolve(response({}, 401)));
    const { api, onUnauthorized } = setup(fetcher);
    await expect(api("/api/login", "POST", {})).rejects.toMatchObject({ status: 401 });
    expect(onUnauthorized).not.toHaveBeenCalled();
    await expect(api("/api/overview")).rejects.toMatchObject({ status: 401 });
    expect(onUnauthorized).toHaveBeenCalledTimes(1);
  });
  it("discards late responses from a closed session", async () => {
    let finish;
    const { api } = setup(
      () =>
        new Promise((resolve) => {
          finish = resolve;
        }),
    );
    const old = api("/api/config");
    api.reset();
    finish(response({ config: { client: "previous tenant" } }));
    await expect(old).rejects.toMatchObject({ name: "AbortError" });
  });
  it("refuses writes without an explicit configuration revision", async () => {
    const fetcher = vi.fn();
    const { api } = setup(fetcher);
    await expect(api("/api/config", "PUT", {})).rejects.toThrow(
      translate("en", "error.missingRevision"),
    );
    expect(fetcher).not.toHaveBeenCalled();
  });
});
