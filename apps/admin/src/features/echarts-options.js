import { graphic, init, use as registerCharts } from "echarts/core";
import { LineChart } from "echarts/charts";
import {
  GridComponent,
  TooltipComponent,
  DataZoomSliderComponent,
  LegendComponent,
  MarkLineComponent,
  MarkPointComponent,
} from "echarts/components";
import { CanvasRenderer } from "echarts/renderers";
import { isMetric, preciseTime, timeLabel } from "./monitor.js";
import { visibleRange } from "./metric-chart.js";

registerCharts([
  LineChart,
  GridComponent,
  TooltipComponent,
  DataZoomSliderComponent,
  LegendComponent,
  MarkLineComponent,
  MarkPointComponent,
  CanvasRenderer,
]);
export { init };

export function readChartTheme(element) {
  const css = getComputedStyle(element);
  const token = (name) => css.getPropertyValue(`--rui-color-${name}`).trim();
  return {
    blue: token("blue"),
    teal: token("teal"),
    dark: token("dark"),
    secondary: token("secondary"),
    muted: token("muted"),
    border: token("border"),
    font: css.fontFamily,
  };
}

export function chartOption({
  tag,
  frames,
  series,
  range,
  zoom,
  ceiling,
  format,
  current,
  hidden,
  theme,
  reducedMotion,
}) {
  const visible = visibleRange(range, zoom);
  const firstVisible = series.find((line) => !hidden.includes(line.key));
  return {
    animation: !reducedMotion,
    animationDuration: 350,
    animationDurationUpdate: 0,
    textStyle: { fontFamily: theme.font },
    grid: { left: 68, right: 22, top: 18, bottom: 76 },
    legend: {
      show: false,
      selected: Object.fromEntries(series.map((line) => [line.name, !hidden.includes(line.key)])),
    },
    xAxis: {
      type: "time",
      min: range[0],
      max: range[1],
      boundaryGap: false,
      splitNumber: 3,
      axisLine: { show: false },
      axisTick: { show: false },
      axisLabel: {
        color: theme.secondary,
        fontSize: 11,
        margin: 14,
        hideOverlap: true,
        formatter: (value) => timeLabel(value, tag),
      },
      axisPointer: {
        show: true,
        snap: true,
        type: "line",
        lineStyle: { opacity: 0 },
        label: { show: false, formatter: ({ value }) => preciseTime(value, tag) },
      },
    },
    yAxis: {
      type: "value",
      min: 0,
      max: ceiling,
      splitNumber: 4,
      interval: ceiling / 4,
      axisLine: { show: false },
      axisTick: { show: false },
      axisLabel: { color: theme.secondary, fontSize: 11, margin: 12, formatter: format },
      splitLine: { lineStyle: { color: "#edf0f2", type: [3, 5] } },
    },
    tooltip: {
      trigger: "axis",
      triggerOn: "none",
      renderMode: "richText",
      confine: true,
      transitionDuration: 0,
      hideDelay: 0,
      borderWidth: 0,
      borderRadius: 12,
      padding: [12, 16],
      backgroundColor: theme.dark,
      shadowBlur: 0,
      shadowColor: "transparent",
      shadowOffsetX: 0,
      shadowOffsetY: 0,
      axisPointer: { type: "line", lineStyle: { opacity: 0 }, label: { show: false } },
      borderColor: theme.dark,
      textStyle: { fontFamily: theme.font, fontSize: 12, fontWeight: 500, color: "#ffffff" },
    },
    dataZoom: [
      {
        id: "timeline",
        type: "slider",
        xAxisIndex: 0,
        filterMode: "none",
        startValue: visible[0],
        endValue: visible[1],
        rangeMode: ["value", "value"],
        left: 68,
        right: 22,
        bottom: 10,
        height: 22,
        minValueSpan: 10000,
        brushSelect: false,
        showDetail: false,
        realtime: true,
        throttle: 40,
        borderColor: "transparent",
        backgroundColor: "#f4f5f7",
        fillerColor: "rgba(73,79,223,0.10)",
        handleSize: "110%",
        handleStyle: { color: "#ffffff", borderColor: "#c9cdd3", borderWidth: 1 },
        moveHandleSize: 0,
        dataBackground: {
          lineStyle: { color: "#c8ccd6", width: 1 },
          areaStyle: { color: "#e4e7ed", opacity: 0.65 },
        },
        selectedDataBackground: {
          lineStyle: { color: theme.blue, opacity: 0.35 },
          areaStyle: { color: theme.blue, opacity: 0.06 },
        },
        emphasis: { handleStyle: { borderColor: theme.blue, color: "#ffffff" } },
      },
    ],
    series: series.map((line) => {
      const color = theme[line.color];
      const value = current?.[line.key];
      return {
        id: line.key,
        name: line.name,
        type: "line",
        color,
        smooth: false,
        showSymbol: frames.rows.length < 3,
        symbol: "circle",
        symbolSize: 6,
        encode: { x: 0, y: 1, tooltip: [1] },
        tooltip: {
          valueFormatter: (value) => line.format(Array.isArray(value) ? value.at(-1) : value),
        },
        connectNulls: false,
        clip: true,
        lineStyle: { color, width: 2.5, cap: "round", join: "round" },
        itemStyle: { color },
        areaStyle: {
          color: new graphic.LinearGradient(0, 0, 0, 1, [
            {
              offset: 0,
              color: line.color === "teal" ? "rgba(0,168,126,0.13)" : "rgba(73,79,223,0.14)",
            },
            {
              offset: 1,
              color: line.color === "teal" ? "rgba(0,168,126,0.01)" : "rgba(73,79,223,0.01)",
            },
          ]),
        },
        emphasis: { disabled: true },
        data: frames.rows.map((sample) => [
          sample.timestamp_ms,
          isMetric(sample[line.key]) ? sample[line.key] : null,
        ]),
        markLine: {
          silent: true,
          animation: false,
          symbol: ["none", "none"],
          label: { show: false },
          lineStyle: { color: "#aeb6c2", width: 1, type: [3, 4] },
          data: current && line === firstVisible ? [{ xAxis: current.timestamp_ms }] : [],
        },
        markPoint: {
          silent: true,
          animation: false,
          symbol: "circle",
          symbolSize: 8,
          label: { show: false },
          itemStyle: { color, borderColor: "#ffffff", borderWidth: 2 },
          data: isMetric(value) ? [{ coord: [current.timestamp_ms, value] }] : [],
        },
      };
    }),
  };
}
