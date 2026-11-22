import { Clock3, RotateCcw, ZoomIn } from "lucide-react";
import { useEffect, useId, useMemo, useRef, useState } from "react";
import { useLocale } from "../app/locale.jsx";
import { chartOption, init, readChartTheme } from "../features/echarts-options.js";
import { chartFrames, metricStats, visibleRange, zoomIn } from "../features/metric-chart.js";
import { isMetric, nearestIndex, preciseTime } from "../features/monitor.js";

export default function MetricChart({
  title,
  description,
  icon: Icon,
  samples,
  series,
  range,
  ceiling = 100,
  format,
  interval,
  selected,
  pinned,
  active,
  onHover,
  onPin,
  zoom,
  onZoom,
  detail,
}) {
  const id = useId();
  const container = useRef(null);
  const chart = useRef(null);
  const theme = useRef(null);
  const latest = useRef(null);
  const [hidden, setHidden] = useState([]);
  const { t, tag, list } = useLocale();
  const frames = useMemo(() => chartFrames(samples, interval), [samples, interval]);
  const viewport = visibleRange(range, zoom);
  const current =
    samples[selected] ??
    samples.findLast(
      (sample) => sample.timestamp_ms >= viewport[0] && sample.timestamp_ms <= viewport[1],
    );
  const mainSeries = series.find((line) => !hidden.includes(line.key));
  const statistics = metricStats(samples, mainSeries.key, viewport);

  useEffect(() => {
    latest.current = { samples, range, viewport, pinned, active, onHover, onPin, onZoom };
  });

  useEffect(() => {
    const element = container.current;
    const instance = init(element, null, { renderer: "canvas" });
    chart.current = instance;
    theme.current = readChartTheme(element);
    function sampleAt(event) {
      const props = latest.current;
      if (!instance.containPixel({ gridIndex: 0 }, [event.offsetX, event.offsetY])) return null;
      const timestamp = instance.convertFromPixel({ xAxisIndex: 0 }, event.offsetX);
      const index = nearestIndex(props.samples, timestamp);
      const sample = props.samples[index];
      return sample &&
        sample.timestamp_ms >= props.viewport[0] &&
        sample.timestamp_ms <= props.viewport[1]
        ? sample
        : null;
    }
    const move = (event) => {
      const props = latest.current;
      if (props.pinned) return;
      const sample = sampleAt(event);
      if (sample) props.onHover(sample.timestamp_ms);
      else if (props.active) props.onHover(null);
    };
    const leave = () => {
      const props = latest.current;
      if (!props.pinned && props.active) props.onHover(null);
    };
    const click = (event) => {
      const sample = sampleAt(event);
      if (sample) latest.current.onPin(sample.timestamp_ms);
    };
    const dataZoom = (event) => {
      const props = latest.current;
      const bounds = event.batch?.[0] ?? event;
      if (!isMetric(bounds.start) || !isMetric(bounds.end)) return;
      const span = props.range[1] - props.range[0];
      props.onZoom(
        bounds.start <= 0 && bounds.end >= 100
          ? null
          : [
              props.range[0] + (span * bounds.start) / 100,
              props.range[0] + (span * bounds.end) / 100,
            ],
      );
    };
    instance.getZr().on("mousemove", move);
    instance.getZr().on("globalout", leave);
    instance.getZr().on("click", click);
    instance.on("datazoom", dataZoom);
    const resize = new ResizeObserver(() => {
      if (!instance.isDisposed()) instance.resize({ silent: true });
    });
    resize.observe(element);
    document.fonts.ready.then(() => {
      if (!instance.isDisposed()) instance.resize({ silent: true });
    });
    return () => {
      resize.disconnect();
      instance.dispose();
      chart.current = null;
    };
  }, []);

  useEffect(() => {
    const instance = chart.current;
    if (!instance) return;
    instance.setOption(
      chartOption({
        title,
        tag,
        frames,
        samples,
        series,
        range,
        zoom,
        ceiling,
        format,
        current,
        hidden,
        theme: theme.current,
        reducedMotion: matchMedia("(prefers-reduced-motion: reduce)").matches,
      }),
      { notMerge: false, lazyUpdate: false, silent: true },
    );
    if (active && current) {
      const index = samples.findIndex((sample) => sample.timestamp_ms === current.timestamp_ms);
      instance.dispatchAction({
        type: "showTip",
        seriesIndex: series.indexOf(mainSeries),
        dataIndex: frames.indices[index],
      });
    } else instance.dispatchAction({ type: "hideTip" });
  }, [
    title,
    tag,
    frames,
    samples,
    series,
    range,
    zoom,
    ceiling,
    format,
    current,
    hidden,
    active,
    mainSeries,
  ]);

  function keyboard(event) {
    const visible = samples.filter(
      (sample) => sample.timestamp_ms >= viewport[0] && sample.timestamp_ms <= viewport[1],
    );
    let index = visible.findIndex((sample) => sample.timestamp_ms === current?.timestamp_ms);
    if (index < 0) index = visible.length - 1;
    if (event.key === "ArrowLeft") index--;
    else if (event.key === "ArrowRight") index++;
    else if (event.key === "Home") index = 0;
    else if (event.key === "End") index = visible.length - 1;
    else if (event.key === "Escape") {
      onPin(null);
      onHover(null);
      return;
    } else return;
    event.preventDefault();
    const sample = visible[Math.max(0, Math.min(visible.length - 1, index))];
    if (sample) onPin(sample.timestamp_ms);
  }

  function toggle(line) {
    setHidden((previous) =>
      previous.includes(line.key)
        ? previous.filter((key) => key !== line.key)
        : previous.length < series.length - 1
          ? [...previous, line.key]
          : previous,
    );
  }

  return (
    <article
      className={`metric-chart ${pinned ? "metric-chart-pinned" : ""}`}
      aria-labelledby={`${id}-title`}
    >
      <div className="metric-chart-head">
        <div className="metric-heading">
          <span className={`metric-icon ${series[0].color}`}>
            <Icon size={20} strokeWidth={1.5} />
          </span>
          <div>
            <h2 id={`${id}-title`}>{title}</h2>
            <p>{description}</p>
          </div>
        </div>
        <div className="chart-tools">
          <button
            type="button"
            aria-label={t("chart.zoomInLabel", { name: title })}
            title={t("chart.zoomIn")}
            onClick={() =>
              onZoom(zoomIn(range, zoom, selected >= 0 ? current?.timestamp_ms : undefined))
            }
          >
            <ZoomIn size={17} />
          </button>
          <button
            type="button"
            aria-label={t("chart.zoomResetLabel", { name: title })}
            title={t("chart.zoomReset")}
            disabled={!zoom}
            onClick={() => onZoom(null)}
          >
            <RotateCcw size={16} />
          </button>
        </div>
      </div>
      <div className="metric-current">
        <div className="metric-current-values">
          {series
            .filter((line) => !hidden.includes(line.key))
            .map((line) => (
              <div key={line.key}>
                <strong className="metric-head-value">{line.format(current?.[line.key])}</strong>
                {series.length > 1 && <span>{line.name}</span>}
              </div>
            ))}
        </div>
        <span className={`metric-mode ${pinned ? "is-pinned" : ""}`}>
          <i />
          {pinned
            ? t("chart.mode.pinned")
            : selected >= 0
              ? t("chart.mode.sample")
              : zoom
                ? t("chart.mode.windowEnd")
                : t("chart.mode.latest")}
        </span>
      </div>
      <div className="metric-chart-meta">
        <div
          className="metric-legend"
          role="group"
          aria-label={t("chart.legendLabel", { name: title })}
        >
          {series.map((line) =>
            series.length > 1 ? (
              <button
                key={line.key}
                type="button"
                aria-pressed={!hidden.includes(line.key)}
                aria-label={t("chart.seriesLabel", { name: title, series: line.name })}
                onClick={() => toggle(line)}
              >
                <i className={`metric-color ${line.color}`} />
                {line.name}
              </button>
            ) : (
              <span key={line.key}>
                <i className={`metric-color ${line.color}`} />
                {line.name}
              </span>
            ),
          )}
        </div>
        <span className="metric-detail">
          {detail?.(current) ||
            (series.length > 1 ? t("chart.unitBytesPerSecond") : t("chart.unitOverall"))}
        </span>
      </div>
      <div className="metric-canvas-wrap">
        <div
          ref={container}
          className="metric-plot"
          role="group"
          tabIndex={0}
          data-engine="echarts"
          data-sample-count={samples.length}
          data-zoomed={Boolean(zoom)}
          data-visible-start={viewport[0]}
          data-visible-end={viewport[1]}
          aria-label={t("chart.seriesDescription", { name: title })}
          aria-describedby={`${id}-reading`}
          onKeyDown={keyboard}
        />
        {!samples.some((sample) => series.some((line) => isMetric(sample[line.key]))) && (
          <div className="metric-no-data">
            {samples.length > 1 ? t("chart.noData") : t("chart.waitingSamples")}
          </div>
        )}
      </div>
      <div className="metric-inspector" id={`${id}-reading`} aria-live={pinned ? "polite" : "off"}>
        <Clock3 size={13} aria-hidden="true" />
        <span>
          {pinned
            ? t("chart.inspector.pinned")
            : selected >= 0
              ? t("chart.mode.sample")
              : t("chart.inspector.recent")}
        </span>
        {current ? (
          <time dateTime={new Date(current.timestamp_ms).toISOString()}>
            {preciseTime(current.timestamp_ms, tag)}
          </time>
        ) : (
          <span>{t("monitor.caption.waiting")}</span>
        )}
        <span className="sr-only">
          {list(
            series
              .filter((line) => !hidden.includes(line.key))
              .map((line) => `${line.name} ${line.format(current?.[line.key])}`),
          )}
        </span>
      </div>
      <div className="metric-statistics" aria-label={t("chart.statisticsLabel", { name: title })}>
        {[
          [t("chart.stat.min"), statistics.min],
          [t("chart.stat.average"), statistics.average],
          [t("chart.stat.max"), statistics.max],
        ].map(([name, value]) => (
          <div key={name}>
            <span>
              {mainSeries.name} · {name}
            </span>
            <strong>{mainSeries.format(value)}</strong>
          </div>
        ))}
      </div>
    </article>
  );
}
