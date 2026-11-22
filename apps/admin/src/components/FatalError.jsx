import React from "react";
import { useLocale } from "../app/locale.jsx";

/**
 * The last-resort screen when a render throws.
 *
 * It lives above the provider boundary's children but inside the locale
 * provider, so it can still speak the reader's language — a crash is exactly
 * when a message in the wrong language is least useful.
 */
export class FatalError extends React.Component {
  state = { failed: false };
  static getDerivedStateFromError() {
    return { failed: true };
  }
  render() {
    if (!this.state.failed) return this.props.children;
    return <Failure />;
  }
}

function Failure() {
  const { t } = useLocale();
  return (
    <main className="fatal-error">
      <h1>{t("fatal.title")}</h1>
      <p>{t("fatal.description")}</p>
      <button className="button button-primary" onClick={() => window.location.reload()}>
        {t("fatal.reload")}
      </button>
    </main>
  );
}
