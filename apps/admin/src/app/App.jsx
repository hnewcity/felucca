import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  Check,
  ChevronRight,
  LogOut,
  Menu,
  PanelLeftClose,
  PanelLeftOpen,
  RefreshCw,
  TriangleAlert,
  X,
} from "lucide-react";
import { createApi } from "../lib/api.js";
import { translate } from "../lib/i18n.js";
import { useLocale } from "./locale.jsx";
import {
  Brand,
  Button,
  Empty,
  Info,
  Loading,
  PageHeader,
  PageSkeleton,
} from "../components/ui.jsx";
import { ConfirmationDialog, EditorDialog } from "../components/Dialogs.jsx";
import { LanguageSwitch } from "../components/LanguageSwitch.jsx";
import Login from "../pages/Login.jsx";
import Overview from "../pages/Overview.jsx";
import Tunnels from "../pages/Services.jsx";
import Devices from "../pages/Devices.jsx";
import Sharing from "../pages/Sharing.jsx";
import Settings from "../pages/Settings.jsx";
import Resources from "../pages/Resources.jsx";
import Config from "../pages/Config.jsx";
import Monitor from "../pages/Monitor.jsx";
import Diagnostics from "../pages/Diagnostics.jsx";
import Deployment from "../pages/ConnectDevice.jsx";
import AdvancedClient from "../pages/Deployment.jsx";
import Platform from "../pages/Platform.jsx";
import { AdminContext } from "./context.jsx";
import { initialPage, pageHeading, pages } from "./navigation.js";
import { loadPage } from "./load-page.js";
import { revealInvalidField } from "../features/config-presentation.js";

const SIDEBAR_STORAGE_KEY = "felucca.sidebar";
// The breakpoint where the stylesheet turns the sidebar into a drawer.
const SIDEBAR_RAIL_QUERY = "(min-width: 1024px)";

function readStoredCollapsed() {
  try {
    return window.localStorage.getItem(SIDEBAR_STORAGE_KEY) === "collapsed";
  } catch {
    // A browser with storage disabled still gets a sidebar, just not a memory.
    return false;
  }
}

/** Whether the sidebar is a rail rather than a drawer, following the viewport. */
function useSidebarRail() {
  const [railed, setRailed] = useState(() => window.matchMedia(SIDEBAR_RAIL_QUERY).matches);
  useEffect(() => {
    const query = window.matchMedia(SIDEBAR_RAIL_QUERY);
    const changed = (event) => setRailed(event.matches);
    setRailed(query.matches);
    query.addEventListener("change", changed);
    return () => query.removeEventListener("change", changed);
  }, []);
  return railed;
}

export default function App() {
  const [session, setSession] = useState(null);
  const sessionRef = useRef(null);
  const apiRef = useRef(null);
  const localeRef = useRef(null);
  const [booting, setBooting] = useState(true);
  const [page, setPage] = useState(initialPage);
  const [client, selectClient] = useState("");
  const [offset, setOffset] = useState(0);
  const [reload, setReload] = useState(0);
  const [result, setData] = useState(null);
  const [loadError, setLoadError] = useState("");
  const [notice, setNotice] = useState(null);
  const [editor, setEditor] = useState(null);
  const [confirmation, setConfirmation] = useState(null);
  const [busy, setBusy] = useState(false);
  const running = useRef(false);
  const [sidebar, setSidebar] = useState(false);
  const [collapsed, setCollapsed] = useState(readStoredCollapsed);
  // A drawer cannot also be a rail: below the breakpoint the sidebar covers the
  // page, so the collapsed choice is kept for when the rail comes back.
  const railed = useSidebarRail();
  const rail = collapsed && railed;
  const toggleSidebar = useCallback(() => {
    setCollapsed((value) => {
      const next = !value;
      try {
        window.localStorage.setItem(SIDEBAR_STORAGE_KEY, next ? "collapsed" : "expanded");
      } catch {
        // Collapsing the sidebar must not fail because storage is unavailable.
      }
      return next;
    });
  }, []);
  const [connection, setConnection] = useState("connecting");
  const [remoteRevision, setRemoteRevision] = useState(null);
  const [ownRevision, setOwnRevision] = useState(null);
  const [loadedAt, setLoadedAt] = useState(null);
  const login = useCallback((next) => {
    sessionRef.current = next;
    setSession(next);
  }, []);
  const clearSession = useCallback(() => {
    apiRef.current?.reset();
    sessionRef.current = null;
    setSession(null);
    setData(null);
    setEditor(null);
    setConfirmation(null);
    setNotice(null);
    setRemoteRevision(null);
    setOwnRevision(null);
    setConnection("connecting");
  }, []);
  const api = useMemo(() => {
    const client = createApi({
      getSession: () => sessionRef.current,
      onUnauthorized: clearSession,
      getLocale: () => localeRef.current,
      translate: (key, args) => translate(localeRef.current, key, args),
    });
    return client;
  }, [clearSession]);
  useEffect(() => {
    apiRef.current = api;
  }, [api]);
  const notify = useCallback((message, error = false) => setNotice({ message, error }), []);
  const refresh = useCallback(
    (message) => {
      setReload((value) => value + 1);
      if (typeof message === "string") notify(message);
    },
    [notify],
  );
  const navigate = useCallback((next, chosen) => {
    if (chosen !== undefined) selectClient(chosen);
    if (next !== "node") setOffset(0);
    window.location.hash = next;
    setPage(next);
    setSidebar(false);
    setNotice(null);
    document.getElementById("main-content")?.focus();
  }, []);
  const run = useCallback(
    async (action) => {
      if (running.current) return;
      running.current = true;
      setBusy(true);
      try {
        await action();
      } catch (error) {
        if (error.name !== "AbortError") notify(error.message, true);
      } finally {
        running.current = false;
        setBusy(false);
      }
    },
    [notify],
  );
  useEffect(() => {
    let active = true;
    const abort = new AbortController();
    api("/api/session", "GET", undefined, { signal: abort.signal })
      .then((value) => {
        if (active) login(value);
      })
      .catch(() => {})
      .finally(() => {
        if (active) setBooting(false);
      });
    return () => {
      active = false;
      abort.abort();
    };
  }, [api, login]);
  useEffect(() => {
    const changed = () => {
      setPage(initialPage());
      setSidebar(false);
    };
    window.addEventListener("hashchange", changed);
    return () => window.removeEventListener("hashchange", changed);
  }, []);
  useEffect(() => {
    if (!session) return;
    const events = new EventSource("/api/events");
    events.onopen = () => setConnection("connected");
    events.onerror = () => {
      setConnection("reconnecting");
      // SSE cannot report an HTTP authentication failure to the browser. A
      // session check turns revoked/expired cookies into an explicit login.
      api("/api/session").catch(() => {});
    };
    events.addEventListener("configuration", (event) => {
      try {
        setRemoteRevision(JSON.parse(event.data).revision);
      } catch {
        /* An incomplete event is ignored; page reads remain authoritative. */
      }
    });
    return () => {
      events.close();
    };
  }, [api, session]);
  const safePage =
    session &&
    ((pages[page]?.admin && session.principal.role !== "admin") ||
      (page === "deployment" && session.principal.role === "viewer"))
      ? "overview"
      : page;
  const { t, time, locale } = useLocale();
  localeRef.current = locale;
  const requestKey = `${safePage}-${client}-${offset}-${reload}`;
  // Moving to another page shows that page's skeleton at once. Re-reading the
  // same page (refresh, paging) keeps it in place, dimmed, until the new data
  // arrives, so the layout does not jump.
  const loading = result?.key !== requestKey && !loadError;
  const switching = loading && result?.page !== safePage;
  const pending = loading && !switching;
  const data = switching ? null : (result?.data ?? null);
  const shownPage = switching ? safePage : (result?.page ?? safePage);
  useEffect(() => {
    // The title follows the page and the language; the page itself is data, so
    // switching language must not re-read it.
    document.title = `${t(pages[safePage].label)} · Felucca`;
  }, [safePage, t]);
  useEffect(() => {
    if (!session) return;
    const abort = new AbortController();
    let active = true;
    setLoadError("");
    loadPage(api, safePage, client, offset, abort.signal, t, session.principal.role)
      .then((value) => {
        if (active) {
          setData({ key: requestKey, page: safePage, data: value });
          setLoadedAt(new Date());
        }
      })
      .catch((error) => {
        if (active && error.name !== "AbortError") setLoadError(error.message);
      });
    return () => {
      active = false;
      abort.abort();
    };
  }, [api, session, safePage, client, offset, requestKey, t]);
  if (booting)
    return (
      <div className="boot-screen">
        <Brand />
        <Loading />
      </div>
    );
  if (!session) return <Login api={api} onLogin={login} />;
  const meta = pages[safePage];
  const shownMeta = pages[shownPage];
  const heading = pageHeading(safePage);
  const parentPage = meta.secondary ? (safePage === "deployment" ? "clients" : "settings") : null;
  const activePage = parentPage ?? safePage;
  const changed =
    data && remoteRevision !== null && remoteRevision > Math.max(data.revision, ownRevision ?? 0);
  const context = {
    api,
    session,
    navigate,
    selectClient,
    setOffset,
    edit: setEditor,
    confirm: setConfirmation,
    refresh,
    notify,
    run,
    busy,
    mutate: async (path, method, value, revision) => {
      const result = await api(path, method, value, { revision });
      if (Number.isSafeInteger(result.revision)) setOwnRevision(result.revision);
      return result;
    },
  };
  const content = loadError ? (
    <>
      <PageHeader title={t(meta.label)} />
      <Info tone="error">{loadError}</Info>
      <Button icon={RefreshCw} onClick={() => refresh()}>
        {t("action.retry")}
      </Button>
    </>
  ) : !data ? (
    <PageSkeleton
      title={t(heading.title)}
      description={heading.description && t(heading.description)}
      layout={safePage === "overview" || safePage === "monitor" ? safePage : "table"}
    />
  ) : data.unavailable ? (
    <>
      <PageHeader title={t(shownMeta.label)} description={t(shownMeta.description)} />
      <section className="surface">
        <Empty
          icon={shownMeta.icon}
          title={t("platform.disabled.title")}
          description={t("platform.disabled.description")}
        />
      </section>
    </>
  ) : shownPage === "overview" ? (
    <Overview data={data} />
  ) : shownPage === "clients" ? (
    <Devices data={data} />
  ) : shownPage === "shares" ? (
    <Sharing data={data} />
  ) : shownPage === "settings" ? (
    <Settings data={data} />
  ) : shownPage === "tunnels" ? (
    <Tunnels data={data} />
  ) : shownPage === "config" ? (
    <Config data={data} />
  ) : shownPage === "monitor" ? (
    <Monitor data={data} />
  ) : shownPage === "diagnostics" ? (
    <Diagnostics data={data} />
  ) : shownPage === "deployment" ? (
    <Deployment data={data} />
  ) : shownPage === "agent" ? (
    <AdvancedClient data={data} />
  ) : shownPage === "node" ? (
    <Platform data={data} offset={offset} />
  ) : (
    <Resources kind={shownPage} data={data} />
  );
  return (
    <AdminContext.Provider value={context}>
      <a
        className="skip-link"
        href="#main-content"
        onClick={(event) => {
          event.preventDefault();
          document.getElementById("main-content")?.focus();
        }}
      >
        {t("a11y.skipToContent")}
      </a>
      <div
        id="workspace"
        className={`workspace ${rail ? "sidebar-collapsed" : ""}`}
        onInvalidCapture={revealInvalidField}
      >
        {sidebar && (
          <button
            className="sidebar-backdrop"
            aria-label={t("a11y.closeNavigation")}
            onClick={() => setSidebar(false)}
          />
        )}
        <aside className={`sidebar ${sidebar ? "sidebar-open" : ""}`} id="sidebar">
          <a
            href="#overview"
            className="brand-link"
            aria-label={t("a11y.feluccaOverview")}
            onClick={() => navigate("overview")}
          >
            <Brand />
          </a>
          <nav aria-label={t("a11y.managementSections")}>
            {["nav.group.operations", "nav.group.administration"].map((group) => (
              <div className="nav-group" key={group}>
                <p>{t(group)}</p>
                {Object.entries(pages)
                  .filter(
                    ([, item]) =>
                      !item.secondary &&
                      item.group === group &&
                      (!item.admin || session.principal.role === "admin"),
                  )
                  .map(([key, item]) => {
                    const Icon = item.icon;
                    return (
                      <button
                        key={key}
                        className={`nav-item ${key === activePage ? "active" : ""}`}
                        aria-current={key === safePage ? "page" : undefined}
                        aria-label={t(item.label)}
                        title={rail ? t(item.label) : undefined}
                        onClick={() => navigate(key)}
                      >
                        <Icon size={19} strokeWidth={1.6} />
                        <span className="nav-label">{t(item.label)}</span>
                        {key === activePage && <span className="nav-dot" />}
                      </button>
                    );
                  })}
              </div>
            ))}
          </nav>
          <div className="sidebar-bottom">
            <LanguageSwitch compact={rail} />
          </div>
        </aside>
        <div className="workspace-main" aria-busy={pending}>
          {pending && <div className="page-progress" aria-hidden="true" />}
          <header className="topbar">
            <div className="breadcrumb">
              <Button
                className="icon-button collapse-toggle"
                aria-label={t(rail ? "a11y.expandSidebar" : "a11y.collapseSidebar")}
                aria-expanded={!rail}
                aria-controls="sidebar"
                icon={rail ? PanelLeftOpen : PanelLeftClose}
                onClick={toggleSidebar}
              />
              <Button
                className="icon-button mobile-menu"
                aria-label={t("a11y.openNavigation")}
                aria-expanded={sidebar}
                aria-controls="sidebar"
                icon={Menu}
                onClick={() => setSidebar(!sidebar)}
              />
              <div className="breadcrumb-path">
                {parentPage ? (
                  <a
                    className="breadcrumb-parent"
                    href={`#${parentPage}`}
                    aria-label={t(parentPage === "settings" ? "flow.backSettings" : "nav.clients")}
                    onClick={(event) => {
                      event.preventDefault();
                      navigate(parentPage);
                    }}
                  >
                    {t(pages[parentPage].label)}
                  </a>
                ) : (
                  <span className="breadcrumb-root">{t("shell.console")}</span>
                )}
                <ChevronRight
                  size={14}
                  aria-hidden="true"
                  className={parentPage ? "" : "breadcrumb-root"}
                />
                <strong aria-current="page">{t(meta.label)}</strong>
              </div>
            </div>
            <div className="topbar-right">
              <div className="page-refresh">
                {data && loadedAt && (
                  <span className="updated-at">
                    {t("status.updatedAt", { time: time(loadedAt) })}
                  </span>
                )}
                <Button
                  className="icon-button"
                  icon={RefreshCw}
                  aria-label={t("action.refreshData")}
                  title={t("action.refreshData")}
                  disabled={!data || pending || Boolean(editor) || Boolean(confirmation) || busy}
                  onClick={() => refresh()}
                />
              </div>
              <div className="account">
                <span className="account-avatar">
                  {session.principal.account.slice(0, 1).toUpperCase()}
                </span>
                <div>
                  <strong>{session.principal.account}</strong>
                  <small>{t(`role.${session.principal.role}`)}</small>
                </div>
              </div>
              <Button
                className="icon-button"
                icon={LogOut}
                aria-label={t("a11y.signOut")}
                disabled={busy}
                onClick={() =>
                  run(async () => {
                    await api("/api/logout", "POST");
                    clearSession();
                  })
                }
              />
            </div>
          </header>
          <main id="main-content" className="main-content" tabIndex={-1}>
            {connection === "reconnecting" && (
              <div className="changed" role="status">
                <TriangleAlert size={18} />
                <span>{t("status.configReconnecting")}</span>
              </div>
            )}
            {changed && !pending && (
              <div className="changed" role="status">
                <TriangleAlert size={18} />
                <span>{t("status.configChanged")}</span>
                <Button
                  disabled={Boolean(editor) || Boolean(confirmation)}
                  onClick={() => refresh()}
                >
                  {t("action.refreshData")}
                </Button>
              </div>
            )}
            {/* A stale page is shown but cannot be used: its actions would act on
                the page the user just left. */}
            <div
              className={`page-content ${pending ? "page-pending" : ""}`}
              key={pending ? result.key : requestKey}
              inert={pending}
            >
              {content}
            </div>
          </main>
        </div>
      </div>
      {notice && (
        <div
          className={`notice ${notice.error ? "notice-error" : ""}`}
          role={notice.error ? "alert" : "status"}
        >
          {notice.error ? <TriangleAlert size={19} /> : <Check size={19} />}
          <span>{notice.message}</span>
          <Button
            className="icon-button"
            icon={X}
            aria-label={t("a11y.dismissNotice")}
            onClick={() => setNotice(null)}
          />
        </div>
      )}
      {editor && (
        <EditorDialog
          editor={editor}
          onClose={() => setEditor(null)}
          onSaved={() => {
            setEditor(null);
            refresh(t("notice.configSaved"));
          }}
        />
      )}
      {confirmation && (
        <ConfirmationDialog confirmation={confirmation} onClose={() => setConfirmation(null)} />
      )}
    </AdminContext.Provider>
  );
}
