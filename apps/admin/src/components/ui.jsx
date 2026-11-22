import { ArrowUpRight, Inbox, LoaderCircle, Search } from "lucide-react";
import { useLocale } from "../app/locale.jsx";

export function Button({
  children,
  variant = "secondary",
  icon: Icon,
  className = "",
  busy,
  disabled,
  ...props
}) {
  return (
    <button
      type="button"
      className={`button button-${variant} ${className}`}
      disabled={disabled || busy}
      {...props}
    >
      {busy ? (
        <LoaderCircle size={17} className="spin" aria-hidden="true" />
      ) : Icon ? (
        <Icon size={17} aria-hidden="true" />
      ) : null}
      {children}
    </button>
  );
}
export function Badge({ children, tone = "neutral", dot = true }) {
  return (
    <span className={`badge badge-${tone}`}>
      {dot && <i aria-hidden="true" />}
      {children}
    </span>
  );
}
export function Empty({ title, description, action, icon: Icon = Inbox }) {
  const { t } = useLocale();
  return (
    <div className="empty">
      <div className="empty-icon">
        <Icon size={28} strokeWidth={1.4} />
      </div>
      <h3>{title ?? t("common.noRecords")}</h3>
      {description && <p>{description}</p>}
      {action}
    </div>
  );
}
export function Loading() {
  const { t } = useLocale();
  return (
    <div className="loading" role="status">
      <LoaderCircle className="spin" size={25} />
      <span>{t("common.loading")}</span>
    </div>
  );
}
// Placeholder shapes in the page's own layout. The real title is known before
// the data is, so it appears at once and the content fills in beneath it.
export function PageSkeleton({ title, description, layout }) {
  const { t } = useLocale();
  return (
    <div className="page-skeleton" role="status" aria-label={t("common.loading")}>
      <PageHeader title={title} description={description} />
      {layout === "overview" ? (
        <>
          <div className="stats-grid">
            {[0, 1, 2].map((index) => (
              <div className="skeleton-block skeleton-stat" key={index} />
            ))}
          </div>
          <div className="overview-grid">
            <div className="skeleton-block skeleton-panel" />
            <div className="skeleton-block skeleton-panel" />
          </div>
        </>
      ) : layout === "monitor" ? (
        <>
          <div className="skeleton-block skeleton-strip" />
          <div className="metric-grid">
            {[0, 1, 2, 3].map((index) => (
              <div className="skeleton-block skeleton-chart" key={index} />
            ))}
          </div>
        </>
      ) : (
        <section className="surface skeleton-table">
          {[0, 1, 2, 3, 4, 5].map((index) => (
            <div className="skeleton-row" key={index}>
              <span className="skeleton-line" />
              <span className="skeleton-line" />
              <span className="skeleton-line" />
            </div>
          ))}
        </section>
      )}
    </div>
  );
}
export function PageHeader({ title, description, children }) {
  return (
    <div className="page-head">
      <div>
        <h1 id="page-title">{title}</h1>
        {description && <p className="page-description">{description}</p>}
      </div>
      {children && <div className="page-actions">{children}</div>}
    </div>
  );
}
export function SectionHeader({ title, description, children }) {
  return (
    <div className="section-head">
      <div>
        <h2>{title}</h2>
        {description && <p>{description}</p>}
      </div>
      {children && <div className="section-actions">{children}</div>}
    </div>
  );
}
export function SearchInput({ value, onChange, placeholder }) {
  const { t } = useLocale();
  return (
    <label className="search">
      <Search size={18} aria-hidden="true" />
      <input
        type="search"
        aria-label={placeholder ?? t("common.searchPlaceholder")}
        placeholder={placeholder ?? t("common.searchPlaceholder")}
        value={value}
        onChange={(event) => onChange(event.target.value)}
      />
    </label>
  );
}
export function DataTable({ headers, rows, label }) {
  const { t } = useLocale();
  return (
    <div
      className="table-wrap"
      role="region"
      aria-label={label || t("common.dataList")}
      tabIndex={0}
    >
      <table>
        <thead>
          <tr>
            {headers.map((header, index) => (
              <th key={index} scope="col">
                {header}
              </th>
            ))}
          </tr>
        </thead>
        <tbody>
          {rows.map((row) => (
            <tr key={row.id}>
              {row.cells.map((cell, index) => (
                <td key={index}>{cell ?? "—"}</td>
              ))}
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}
export function TextLink({ children, onClick }) {
  return (
    <button className="text-link" onClick={onClick}>
      {children}
      <ArrowUpRight size={16} aria-hidden="true" />
    </button>
  );
}
export function Brand({ light = false }) {
  return (
    <span className={`brand ${light ? "brand-light" : ""}`}>
      <svg width="30" height="32" viewBox="0 0 40 42" fill="none" aria-hidden="true">
        <path d="M4 30 22 1v29H4Zm22 0V12l10 18H26ZM3 35h34l-6 5H9l-6-5Z" fill="currentColor" />
      </svg>
      <span>
        felucca<span className="brand-dot">.</span>
      </span>
    </span>
  );
}
export function Info({ children, tone = "" }) {
  return (
    <div className={`info ${tone}`} role={tone === "error" ? "alert" : undefined}>
      {children}
    </div>
  );
}
