import { encode } from "../lib/format.js";

// Pages that read the overview before anything else: integrations need to know
// whether the node is enabled, diagnostics whether profiling is reported.
const OVERVIEW_FIRST = ["node", "platforms", "webhooks", "diagnostics"];

// A page is published only when all configuration reads describe the same
// revision. This keeps table actions tied to the configuration the user saw.
// Reads that do not depend on each other run concurrently, so a page costs the
// slowest request rather than the sum of all of them.
export async function loadPage(api, page, chosenClient, offset, signal, translate, role) {
  const get = (path) => api(path, "GET", undefined, { signal });
  for (let attempt = 0; attempt < 3; attempt++) {
    const overviewRequest = get("/api/overview");
    const snapshot = { chosenClient };
    const revisions = [];
    const read = async (path) => {
      const data = await get(path);
      if (data.revision !== undefined) revisions.push(data.revision);
      return data;
    };
    let overview;
    if (OVERVIEW_FIRST.includes(page)) {
      overview = await overviewRequest;
      if (page !== "diagnostics" && !overview.node)
        return { overview, revision: overview.revision, chosenClient, unavailable: true };
    }
    const readPage = async () => {
      if (page === "diagnostics") {
        const [diagnostics, session] = await Promise.all([
          read("/api/diagnostics"),
          overview.profiling !== undefined ? get("/api/session") : null,
        ]);
        snapshot.diagnostics = diagnostics.diagnostics;
        if (session?.principal.role === "admin")
          snapshot.certificates = (await read("/api/certificates")).diagnostics;
      } else if (page === "deployment" || page === "agent") {
        const [clients, shares, launch, packages] = await Promise.all([
          read("/api/clients"),
          read("/api/shares"),
          read("/api/launch"),
          read("/api/agent-packages"),
        ]);
        snapshot.clients = clients.config;
        snapshot.shares = shares.config;
        snapshot.launch = launch;
        snapshot.packages = packages.packages;
      } else if (page === "tunnels" || page === "shares") {
        // Managed configuration is per client, so only it waits for the client list.
        const managed = async () => {
          const clients = (await read("/api/clients")).config;
          const entries = await Promise.all(
            clients
              .filter((client) => client.managed)
              .map(async (client) => [
                client.id,
                (await read(`/api/managed/${encode(client.id)}`)).config,
              ]),
          );
          return [clients, Object.fromEntries(entries)];
        };
        const [[clients, managedByClient], launch, packages, shares] = await Promise.all([
          managed(),
          read("/api/launch"),
          read("/api/agent-packages"),
          read("/api/shares"),
        ]);
        Object.assign(snapshot, {
          clients,
          managedByClient,
          launch,
          packages: packages.packages,
          shares: shares.config,
        });
      } else if (page === "settings") {
        if (role === "admin") snapshot.config = (await read("/api/config")).config;
      } else if (page === "config") {
        snapshot.config = (await read("/api/config")).config;
      } else if (page === "node") {
        [snapshot.status, snapshot.callbacks, snapshot.changes] = await Promise.all([
          read("/api/node/status"),
          read(`/api/node/callbacks?offset=${offset}&limit=100`),
          read("/api/node/changes"),
        ]);
      } else if (!["overview", "monitor"].includes(page)) {
        const { config } = await read(`/api/${page}`);
        snapshot.rows =
          page === "accounts"
            ? Object.entries(config).map(([id, config]) => ({ id, config }))
            : config;
        // The device editor also edits the destination scope sent to each device.
        if (page === "clients")
          snapshot.managedByClient = Object.fromEntries(
            await Promise.all(
              config
                .filter((client) => client.managed)
                .map(async (client) => [
                  client.id,
                  (await read(`/api/managed/${encode(client.id)}`)).config,
                ]),
            ),
          );
      }
    };
    [overview] = await Promise.all([overviewRequest, readPage()]);
    if (revisions.every((revision) => revision === overview.revision))
      return { ...snapshot, overview, revision: overview.revision };
  }
  throw new Error(translate("error.configUpdating"));
}
