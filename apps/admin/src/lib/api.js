export class ApiError extends Error {
  constructor(message, status) {
    super(message);
    this.name = "ApiError";
    this.status = status;
  }
}

// The console speaks the operator's language and so does the server: every
// request carries it, and the listener answers its own messages in kind.

// One client per session. Failed network requests retain their idempotency key
// so a retry cannot apply an already-committed configuration transaction twice.
export function createApi({ getSession, onUnauthorized, getLocale, translate, fetcher = fetch }) {
  const pending = new Map();
  let generation = 0;
  async function request(path, method = "GET", value, options = {}) {
    const requestGeneration = generation;
    const headers = {
      Accept: options.raw ? "*/*" : "application/json",
      "Accept-Language": getLocale(),
    };
    if (value !== undefined) headers["Content-Type"] = "application/json";
    const csrf = getSession()?.csrf;
    if (csrf) headers["X-Felucca-CSRF"] = csrf;
    const mutation =
      !["GET", "HEAD"].includes(method) &&
      !["/api/login", "/api/logout", "/api/validate", "/api/profile", "/api/token"].includes(path);
    const signature = JSON.stringify([method, path, value, options.revision]);
    if (mutation) {
      if (!Number.isSafeInteger(options.revision))
        throw new Error(translate("error.missingRevision"));
      headers["If-Match"] = `"${options.revision}"`;
      headers["Idempotency-Key"] =
        options.key ??
        pending.get(signature) ??
        Array.from(crypto.getRandomValues(new Uint8Array(24)), (byte) =>
          byte.toString(16).padStart(2, "0"),
        ).join("");
      if (pending.size >= 64) pending.delete(pending.keys().next().value);
      pending.set(signature, headers["Idempotency-Key"]);
    }
    const response = await fetcher(path, {
      method,
      headers,
      credentials: "same-origin",
      body: value === undefined ? undefined : JSON.stringify(value),
      signal: options.signal,
    });
    // Never let a response from a previous session populate this session's UI.
    if (requestGeneration !== generation)
      throw new DOMException(translate("error.sessionSwitched"), "AbortError");
    if (!response.ok) {
      pending.delete(signature);
      const data = await response.json().catch(() => ({}));
      if (response.status === 401 && path !== "/api/login") onUnauthorized();
      const message =
        response.status === 409
          ? translate("error.staleRevision")
          : path === "/api/login" && response.status === 401
            ? translate("error.badCredentials")
            : data.error || translate("error.requestFailed", { status: response.status });
      throw new ApiError(message, response.status);
    }
    // Retain the key even on an unreadable success response: the server may
    // already have committed the transaction.
    const result = options.raw ? response : await response.json();
    if (requestGeneration !== generation)
      throw new DOMException(translate("error.sessionSwitched"), "AbortError");
    pending.delete(signature);
    return result;
  }
  request.reset = () => {
    generation += 1;
    pending.clear();
  };
  return request;
}
