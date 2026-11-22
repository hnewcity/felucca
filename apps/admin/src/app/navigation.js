import {
  Activity,
  ArrowLeftRight,
  Boxes,
  Cable,
  CircleUserRound,
  Gauge,
  Globe2,
  HardDrive,
  Network,
  Send,
  Settings2,
  UsersRound,
} from "lucide-react";

// Labels and descriptions are catalog keys: the sidebar, the page
// header and the browser title all read the same string through `t()`.
export const pages = {
  overview: {
    label: "nav.overview",
    icon: Gauge,
    group: "nav.group.operations",
  },
  clients: {
    label: "nav.clients",
    icon: Boxes,
    group: "nav.group.operations",
    description: "nav.clients.description",
  },
  tunnels: {
    label: "nav.tunnels",
    icon: ArrowLeftRight,
    group: "nav.group.operations",
    description: "nav.tunnels.description",
  },
  shares: {
    label: "nav.shares",
    icon: Cable,
    group: "nav.group.operations",
    description: "nav.shares.description",
  },
  monitor: {
    label: "nav.monitor",
    icon: Activity,
    group: "nav.group.operations",
    description: "nav.monitor.description",
  },
  diagnostics: {
    label: "cap.diagnostics",
    icon: Activity,
    group: "nav.group.operations",
    description: "cap.diagnosticsHint",
  },
  deployment: {
    label: "flow.connectDevice",
    icon: Boxes,
    group: "nav.group.administration",
    description: "cap.deploymentHint",
  },
  users: {
    label: "nav.users",
    icon: UsersRound,
    group: "nav.group.administration",
    description: "nav.users.description",
  },
  locals: {
    label: "nav.locals",
    icon: HardDrive,
    group: "nav.group.administration",
    description: "nav.locals.description",
  },
  accounts: {
    label: "nav.accounts",
    icon: CircleUserRound,
    group: "nav.group.administration",
    admin: true,
    description: "nav.accounts.description",
  },
  config: {
    label: "nav.config",
    icon: Settings2,
    group: "nav.group.administration",
    admin: true,
    description: "nav.config.description",
  },
  node: {
    label: "nav.node",
    icon: Network,
    group: "nav.group.integrations",
    description: "nav.node.description",
  },
  platforms: {
    label: "nav.platforms",
    icon: Globe2,
    group: "nav.group.integrations",
    admin: true,
    description: "nav.platforms.description",
  },
  webhooks: {
    label: "nav.webhooks",
    icon: Send,
    group: "nav.group.integrations",
    description: "nav.webhooks.description",
  },
};
pages.settings = {
  label: "flow.settings",
  description: "flow.settingsHint",
  icon: Settings2,
  group: "nav.group.administration",
};
pages.agent = {
  label: "flow.advancedClient",
  description: "flow.advancedClientHint",
  icon: Boxes,
  group: "nav.group.administration",
  secondary: true,
};
for (const name of [
  "diagnostics",
  "deployment",
  "users",
  "locals",
  "accounts",
  "config",
  "node",
  "platforms",
  "webhooks",
])
  pages[name].secondary = true;
// Some pages title themselves differently from their sidebar entry. The loading
// skeleton uses the same heading so nothing changes when the data arrives.
const headings = {
  overview: { description: "overview.description" },
  clients: { description: "flow.devicesHint" },
  tunnels: { description: "flow.servicesHint" },
  shares: { description: "flow.sharingHint" },
  monitor: { description: "monitor.description" },
  deployment: { description: "flow.connectHint" },
  config: { title: "config.title", description: "config.description" },
};
export function pageHeading(name) {
  return {
    title: headings[name]?.title ?? pages[name].label,
    description: headings[name]?.description ?? pages[name].description,
  };
}
export function initialPage() {
  const name = window.location.hash.slice(1);
  return Object.hasOwn(pages, name) ? name : "overview";
}
