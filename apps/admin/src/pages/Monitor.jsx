import {
  Activity,
  ArrowDownLeft,
  ArrowUpRight,
  Cpu,
  HardDrive,
  MemoryStick,
  Network,
  RefreshCw,
  Server,
} from "lucide-react";
import { lazy, Suspense, useState } from "react";
import { useAdmin } from "../app/context.jsx";
import { useLocale } from "../app/locale.jsx";
import { Badge, Button, Info, PageHeader } from "../components/ui.jsx";
const MetricChart = lazy(() => import("../components/MetricChart.jsx"));
import {
  capacity,
  domain,
  percentage,
  preciseTime,
  rate,
  rateCeiling,
  uptime,
} from "../features/monitor.js";
import { useHostMetrics } from "../features/use-host-metrics.js";
import { bytes, download } from "../lib/format.js";

// Every label and formatter reads the active language and locale, so the cards
// are rebuilt whenever the reader switches either.
const chartCards = (t, tag) => [
  {
    title: t("monitor.chart.cpu"),
    description: t("monitor.chart.cpu.description"),
    icon: Cpu,
    series: [
      { key: "cpu_percent", name: t("monitor.series.usage"), color: "blue", format: percentage },
    ],
  },
  {
    title: t("monitor.chart.memory"),
    description: t("monitor.chart.memory.description"),
    detail: (sample) =>
      `${capacity(sample?.memory_used_bytes, tag)} / ${capacity(sample?.memory_total_bytes, tag)}`,
    icon: MemoryStick,
    series: [
      { key: "memory_percent", name: t("monitor.series.usage"), color: "teal", format: percentage },
    ],
  },
  {
    title: t("monitor.chart.disk"),
    description: t("monitor.chart.disk.description"),
    detail: (sample) =>
      `${capacity(sample?.disk_used_bytes, tag)} / ${capacity(sample?.disk_total_bytes, tag)}`,
    icon: HardDrive,
    series: [
      { key: "disk_percent", name: t("monitor.series.usage"), color: "blue", format: percentage },
    ],
  },
  {
    title: t("monitor.chart.network"),
    description: t("monitor.chart.network.description"),
    icon: Network,
    network: true,
    series: [
      {
        key: "network_rx_bytes_per_sec",
        name: t("monitor.series.receive"),
        color: "blue",
        format: (value) => rate(value, tag),
      },
      {
        key: "network_tx_bytes_per_sec",
        name: t("monitor.series.send"),
        color: "teal",
        format: (value) => rate(value, tag),
      },
    ],
  },
];

export default function Monitor({ data }) {
  const { session } = useAdmin();
  const { t } = useLocale();
  return session.principal.role === "admin" ? (
    <HostMonitor data={data} />
  ) : (
    <>
      <PageHeader title={t("nav.monitor")} description={t("monitor.description")} />
      <Info>{t("monitor.adminOnly")}</Info>
      <TrafficSummary overview={data.overview} />
    </>
  );
}

function HostMonitor({ data }) {
  const { api, run, notify, busy } = useAdmin();
  const { t, tag } = useLocale();
  const [windowSeconds, setWindow] = useState(900);
  const [hover, setHover] = useState(null);
  const [pin, setPin] = useState(null);
  const [zoom, setZoom] = useState(null);
  const { snapshot, error, refreshing, refresh } = useHostMetrics(api, windowSeconds);
  const samples = snapshot?.samples ?? [];
  const latest = samples.at(-1);
  const pinned = pin !== null && samples.some((sample) => sample.timestamp_ms === pin.timestamp);
  const selected = samples.findIndex(
    (sample) => sample.timestamp_ms === (pinned ? pin.timestamp : hover?.timestamp),
  );
  const inspection = pinned ? pin : hover;
  const host = snapshot?.host;
  const charts = chartCards(t, tag);
  function changeZoom(next) {
    setZoom(next);
    setPin(null);
    setHover(null);
  }
  function inspect(title, timestamp) {
    setHover((previous) =>
      timestamp === null
        ? null
        : previous?.timestamp === timestamp && previous.chart === title
          ? previous
          : { chart: title, timestamp },
    );
  }
  const range = domain(
    samples,
    snapshot?.server_time_ms ?? Date.now(),
    snapshot?.windowSeconds ?? windowSeconds,
  );
  const stale =
    snapshot && snapshot.server_time_ms - (latest?.timestamp_ms ?? snapshot.started_at_ms) > 15000;
  const impaired = Boolean(error || stale || snapshot?.sampling_failed);
  return (
    <>
      <div className="monitor-page-heading">
        <PageHeader title={t("monitor.hostTitle")} description={t("monitor.hostDescription")}>
          <Button icon={RefreshCw} busy={refreshing} onClick={refresh}>
            {t("monitor.refresh")}
          </Button>
          {data.overview.profiling && (
            <Button
              icon={Activity}
              busy={busy}
              onClick={() =>
                run(async () => {
                  notify(t("monitor.profiling"));
                  const response = await api("/api/profile", "POST", { seconds: 5 }, { raw: true });
                  download(await response.blob(), "felucca-cpu.svg");
                  notify(t("monitor.profileDownloaded"));
                })
              }
            >
              {t("monitor.sampleCpu", { seconds: 5 })}
            </Button>
          )}
        </PageHeader>
      </div>
      <section className="host-summary" aria-label={t("monitor.hostInfo")}>
        <div className="host-identity">
          <span className="host-icon">
            <Server size={23} strokeWidth={1.5} />
          </span>
          <div>
            <strong>{host?.name || t("monitor.hostFallbackName")}</strong>
            <span>
              {host
                ? `${host.os || host.architecture} · ${t("monitor.logicalCores", { count: host.cpu_cores })}`
                : t("monitor.hostLoading")}
            </span>
          </div>
        </div>
        <div className="host-uptime">
          <span>{t("monitor.hostUptime")}</span>
          <strong>{uptime(latest?.uptime_seconds, t)}</strong>
        </div>
        <Badge tone={impaired ? "warning" : latest ? "success" : "neutral"}>
          {impaired
            ? t("monitor.badge.impaired")
            : latest
              ? t("monitor.badge.live")
              : t("monitor.badge.connecting")}
        </Badge>
      </section>
      {error && <Info tone="error">{t("monitor.updateFailed", { error })}</Info>}
      {(stale || snapshot?.sampling_failed) && <Info>{t("monitor.staleData")}</Info>}
      <div className="monitor-toolbar">
        <div>
          <h2>{t("monitor.trendTitle")}</h2>
          <p>
            {t("monitor.trendNote", { count: samples.length })}
            {zoom ? ` · ${t("monitor.zoomedIn")}` : ""}
          </p>
        </div>
        <div className="monitor-controls">
          {pinned && (
            <Button
              onClick={() => {
                setPin(null);
                setHover(null);
              }}
            >
              {t("monitor.backToLive")}
            </Button>
          )}
          <div className="time-ranges" role="group" aria-label={t("monitor.rangeLabel")}>
            {[
              [300, t("monitor.range.5")],
              [900, t("monitor.range.15")],
              [3600, t("monitor.range.60")],
            ].map(([seconds, label]) => (
              <button
                key={seconds}
                type="button"
                aria-pressed={windowSeconds === seconds}
                onClick={() => {
                  setWindow(seconds);
                  setZoom(null);
                  setPin(null);
                  setHover(null);
                }}
              >
                {label}
              </button>
            ))}
          </div>
        </div>
      </div>
      <Suspense
        fallback={
          <div className="metric-grid" aria-label={t("monitor.loadingCharts")}>
            {charts.map((chart) => (
              <div className="metric-chart-skeleton" key={chart.title}>
                {t("monitor.loadingChart", { name: chart.title })}
              </div>
            ))}
          </div>
        }
      >
        <div className="metric-grid">
          {charts.map((chart) => (
            <MetricChart
              key={chart.title}
              {...chart}
              samples={samples}
              range={range}
              ceiling={
                chart.network
                  ? rateCeiling(
                      samples,
                      chart.series.map((line) => line.key),
                    )
                  : 100
              }
              format={chart.network ? (value) => bytes(value, tag) : (value) => `${value}%`}
              interval={snapshot?.sample_interval_ms ?? 5000}
              selected={selected}
              pinned={pinned}
              active={inspection?.chart === chart.title}
              onHover={(timestamp) => inspect(chart.title, timestamp)}
              onPin={(timestamp) =>
                setPin(timestamp === null ? null : { chart: chart.title, timestamp })
              }
              zoom={zoom}
              onZoom={changeZoom}
            />
          ))}
        </div>
      </Suspense>
      <div className="monitor-caption">
        <span>{t("monitor.caption.hints")}</span>
        <span>
          {t("monitor.caption.timezone")} ·{" "}
          {latest
            ? t("monitor.caption.updated", { time: preciseTime(latest.timestamp_ms, tag) })
            : t("monitor.caption.waiting")}
        </span>
      </div>
      <TrafficSummary overview={data.overview} />
    </>
  );
}

function TrafficSummary({ overview }) {
  const { t, tag } = useLocale();
  const resources = [...overview.clients, ...overview.locals];
  const inlet = resources.reduce((n, resource) => n + (resource.traffic?.inlet_bytes || 0), 0);
  const outlet = resources.reduce((n, resource) => n + (resource.traffic?.export_bytes || 0), 0);
  return (
    <section className="traffic-monitor" aria-label={t("monitor.trafficLabel")}>
      <h2>{t("monitor.trafficTitle")}</h2>
      <p>{t("monitor.trafficDescription")}</p>
      <div className="monitor-stats">
        <div>
          <span>
            <ArrowDownLeft size={18} />
            {t("monitor.cumulativeInbound")}
          </span>
          <strong>{bytes(inlet, tag)}</strong>
        </div>
        <div>
          <span>
            <ArrowUpRight size={18} />
            {t("monitor.cumulativeOutbound")}
          </span>
          <strong>{bytes(outlet, tag)}</strong>
        </div>
        <div>
          <span>
            <Activity size={18} />
            {t("overview.currentConnections")}
          </span>
          <strong>{resources.reduce((n, resource) => n + resource.connections, 0)}</strong>
        </div>
      </div>
    </section>
  );
}
