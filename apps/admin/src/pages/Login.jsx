import { useRef, useState } from "react";
import { ArrowRight, Eye, EyeOff, LockKeyhole } from "lucide-react";
import { Brand, Button } from "../components/ui.jsx";
import { useLocale } from "../app/locale.jsx";

export default function Login({ api, onLogin }) {
  const { t } = useLocale();
  const [busy, setBusy] = useState(false);
  const [visible, setVisible] = useState(false);
  const [error, setError] = useState("");
  const submitting = useRef(false);
  async function submit(event) {
    event.preventDefault();
    if (submitting.current) return;
    submitting.current = true;
    setBusy(true);
    setError("");
    const form = event.currentTarget;
    const data = new FormData(form);
    try {
      const session = await api("/api/login", "POST", {
        account: data.get("account"),
        password: data.get("password"),
      });
      form.reset();
      onLogin(session);
    } catch (error) {
      setError(error.message);
    } finally {
      submitting.current = false;
      setBusy(false);
    }
  }
  return (
    <main id="login" className="login-layout">
      <section className="login-story">
        <Brand light />
        <div className="login-copy">
          <h1>{t("login.consoleTitle")}</h1>
          <p>{t("login.consoleDescription")}</p>
        </div>
        <div className="connection-art" aria-hidden="true">
          <svg viewBox="0 0 480 240" fill="none">
            <ellipse cx="240" cy="120" rx="210" ry="78" stroke="currentColor" />
            <ellipse
              cx="240"
              cy="120"
              rx="210"
              ry="78"
              transform="rotate(-25 240 120)"
              stroke="currentColor"
            />
            <ellipse
              cx="240"
              cy="120"
              rx="210"
              ry="78"
              transform="rotate(25 240 120)"
              stroke="currentColor"
            />
            <path d="M32 120h418M240 25v190" stroke="currentColor" strokeDasharray="3 8" />
            <circle cx="240" cy="120" r="32" fill="#f4f4f4" />
            <path d="m229 131 15-27v27h-15Zm19 0v-16l8 16h-8Z" fill="#191c1f" />
            <circle cx="62" cy="81" r="7" fill="#f4f4f4" />
            <circle cx="427" cy="155" r="7" fill="#00a87e" />
          </svg>
        </div>
      </section>
      <section className="login-panel">
        <div className="mobile-brand">
          <Brand />
        </div>
        <div className="login-form-wrap">
          <div className="login-lock">
            <LockKeyhole size={24} strokeWidth={1.5} />
          </div>
          <h2>{t("login.submit")}</h2>
          <p className="muted">{t("login.panelMuted")}</p>
          <form id="login-form" onSubmit={submit}>
            <label>
              {t("login.account")}
              <input
                name="account"
                autoComplete="username"
                required
                maxLength={128}
                placeholder={t("login.accountPlaceholder")}
                disabled={busy}
              />
            </label>
            <label>
              {t("login.password")}
              <div className="password-input">
                <input
                  name="password"
                  type={visible ? "text" : "password"}
                  autoComplete="current-password"
                  required
                  maxLength={4096}
                  placeholder={t("login.passwordPlaceholder")}
                  disabled={busy}
                />
                <Button
                  className="icon-button"
                  aria-label={t(visible ? "login.hidePassword" : "login.showPassword")}
                  aria-pressed={visible}
                  icon={visible ? EyeOff : Eye}
                  onClick={() => setVisible(!visible)}
                />
              </div>
            </label>
            <p className="form-error" role="alert">
              {error}
            </p>
            <Button type="submit" variant="primary" busy={busy}>
              {t("login.submit")}
              <ArrowRight size={18} aria-hidden="true" />
            </Button>
          </form>
        </div>
      </section>
    </main>
  );
}
