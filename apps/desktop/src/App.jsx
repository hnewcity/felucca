import { useCallback, useEffect, useRef, useState } from "react";
import { replyText, translate } from "./i18n.js";
import { ipc } from "./ipc.js";

const tones = {
  online: "success",
  connecting: "warning",
  failed: "danger",
  stopped: "muted",
};

export default function App() {
  const [locale, setLocale] = useState("en");
  const [connections, setConnections] = useState([]);
  const [text, setText] = useState("");
  const [name, setName] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  // A link opened from a browser waits here until the user accepts it.
  const [offer, setOffer] = useState(null);
  const [autostart, setAutostart] = useState(false);
  // Webviews do not all implement window.confirm, so removal asks in place.
  const [removing, setRemoving] = useState(null);
  const offers = useRef([]);
  const t = useCallback((key, args) => translate(locale, key, args), [locale]);

  const refresh = useCallback(async () => {
    try {
      setConnections(await ipc.list());
    } catch (reply) {
      setError(replyText(locale, reply));
    }
  }, [locale]);

  const nextOffer = useCallback(async () => {
    while (offers.current.length) {
      const link = offers.current.shift();
      try {
        setOffer({ link, ...(await ipc.describe(link)) });
        return;
      } catch (reply) {
        setError(replyText(locale, reply));
      }
    }
    setOffer(null);
  }, [locale]);

  const takeLinks = useCallback(async () => {
    offers.current.push(...(await ipc.takeLinks()));
    if (!offer) await nextOffer();
  }, [nextOffer, offer]);

  useEffect(() => {
    ipc.language().then((code) => {
      setLocale(code);
      document.documentElement.lang = code;
    });
    ipc.autostart().then(setAutostart, () => {});
  }, []);

  useEffect(() => {
    let active = true;
    let timer;
    const poll = async () => {
      await refresh();
      if (active) timer = setTimeout(poll, 2000);
    };
    poll();
    return () => {
      active = false;
      clearTimeout(timer);
    };
  }, [refresh]);

  useEffect(() => {
    takeLinks();
    const stop = ipc.onLinks(() => takeLinks());
    return () => {
      stop.then((unlisten) => unlisten());
    };
  }, [takeLinks]);

  async function add(source, label) {
    setBusy(true);
    setError("");
    try {
      await ipc.add(source, label);
      await refresh();
      return true;
    } catch (reply) {
      setError(replyText(locale, reply));
      return false;
    } finally {
      setBusy(false);
    }
  }

  async function submit(event) {
    event.preventDefault();
    if (await add(text, name)) {
      setText("");
      setName("");
    }
  }

  async function accept() {
    await add(offer.link, "");
    await nextOffer();
  }

  async function act(action) {
    setError("");
    try {
      await action();
      await refresh();
    } catch (reply) {
      setError(replyText(locale, reply));
    }
  }

  async function toggleAutostart(enabled) {
    setError("");
    try {
      await ipc.setAutostart(enabled);
      setAutostart(await ipc.autostart());
    } catch (reply) {
      setError(replyText(locale, reply));
    }
  }

  return (
    <main>
      <header>
        <h1>{t("app.title")}</h1>
        <p>{t("app.subtitle")}</p>
      </header>

      {offer && (
        <section
          className="surface offer"
          role="dialog"
          aria-labelledby="offer-title"
        >
          <h2 id="offer-title">{t("confirm.title")}</h2>
          <dl>
            <dt>{t("confirm.server")}</dt>
            <dd>{offer.server}</dd>
            <dt>{t("confirm.device")}</dt>
            <dd>{offer.device}</dd>
          </dl>
          <p className="hint">{t("confirm.hint")}</p>
          <div className="actions">
            <button className="primary" disabled={busy} onClick={accept}>
              {t("confirm.accept")}
            </button>
            <button disabled={busy} onClick={nextOffer}>
              {t("confirm.cancel")}
            </button>
          </div>
        </section>
      )}

      <section className="surface">
        <h2>{t("list.title")}</h2>
        {connections.length === 0 ? (
          <p className="hint">{t("list.empty")}</p>
        ) : (
          <ul className="connections">
            {connections.map((connection) => (
              <li key={connection.id}>
                <div className="connection">
                  <strong>{connection.name}</strong>
                  <span className="hint">
                    {connection.server}
                    {connection.device && ` · ${connection.device}`}
                  </span>
                  {connection.error && (
                    <span className="form-error">
                      {replyText(locale, connection.error)}
                    </span>
                  )}
                </div>
                <span className={`badge ${tones[connection.status]}`}>
                  {t(`status.${connection.status}`)}
                </span>
                <div className="actions">
                  <button
                    onClick={() =>
                      act(() =>
                        ipc.setEnabled(connection.id, !connection.enabled),
                      )
                    }
                  >
                    {t(connection.enabled ? "action.disable" : "action.enable")}
                  </button>
                  {removing === connection.id ? (
                    <>
                      <span className="hint">
                        {t("action.removeConfirm", { name: connection.name })}
                      </span>
                      <button
                        className="danger"
                        onClick={() => {
                          setRemoving(null);
                          act(() => ipc.remove(connection.id));
                        }}
                      >
                        {t("action.remove")}
                      </button>
                      <button onClick={() => setRemoving(null)}>
                        {t("confirm.cancel")}
                      </button>
                    </>
                  ) : (
                    <button
                      className="danger"
                      onClick={() => setRemoving(connection.id)}
                    >
                      {t("action.remove")}
                    </button>
                  )}
                </div>
              </li>
            ))}
          </ul>
        )}
      </section>

      <form className="surface" onSubmit={submit}>
        <h2>{t("add.title")}</h2>
        <p className="hint">{t("add.hint")}</p>
        <label>
          {t("add.placeholder")}
          <textarea
            required
            rows={3}
            spellCheck={false}
            autoComplete="off"
            value={text}
            onChange={(event) => setText(event.target.value)}
          />
        </label>
        <label>
          {t("add.name")}
          <input
            maxLength={80}
            placeholder={t("add.namePlaceholder")}
            value={name}
            onChange={(event) => setName(event.target.value)}
          />
        </label>
        <button className="primary" type="submit" disabled={busy}>
          {busy ? t("add.check") : t("add.submit")}
        </button>
      </form>

      <p className="form-error" role="alert">
        {error}
      </p>

      <section className="surface">
        <label className="switch">
          <input
            type="checkbox"
            checked={autostart}
            onChange={(event) => toggleAutostart(event.target.checked)}
          />
          <span>
            <strong>{t("settings.autostart")}</strong>
            <small className="hint">{t("settings.autostartHint")}</small>
          </span>
        </label>
      </section>
    </main>
  );
}
