#!/usr/bin/env python3
"""Form acceptance in installed Chrome against newly built server assets.

Requires the optional Playwright dependency from scripts/probes/requirements.txt.
Starts an isolated, loopback-only fixture and removes no existing configuration.
"""
import asyncio
import json
import os
import secrets
import socket
from pathlib import Path
from urllib.request import urlopen

from playwright.async_api import async_playwright, expect

ROOT = Path(__file__).resolve().parents[2]
OUTPUT = Path(os.environ.get("FELUCCA_PROBE_OUTPUT", ROOT / "target/tmp/form-design"))
TEXT = json.loads((ROOT / "apps/admin/src/locales/zh.json").read_text())


async def exercise(fixture):
    origin = fixture["origin"]
    async with async_playwright() as p:
        browser = await p.chromium.launch(
            executable_path=os.environ.get("FELUCCA_CHROME", "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"),
            headless=True, ignore_default_args=["--hide-scrollbars"],
        )
        context = await browser.new_context(viewport={"width": 1440, "height": 1000})
        await context.add_init_script("localStorage.setItem('felucca.locale', 'zh')")
        page = await context.new_page()
        errors, writes, checks = [], [], []
        page.on("pageerror", lambda e: errors.append(str(e)))
        page.on("request", lambda r: writes.append(r.url) if r.method in ["PUT", "POST"] else None)
        try:
            await page.goto(origin)
            await page.locator('[name="account"]').fill("root")
            await page.locator('[name="password"]').fill(fixture["password"])
            await page.locator('#login-form button[type="submit"]').click()
            await page.locator("#workspace").wait_for()
            session = await (await context.request.get(origin + "/api/session")).json()
            overview = await (await context.request.get(origin + "/api/overview")).json()
            key = secrets.token_hex(32)
            tunnel = {"id": 1, "mode": "tcp", "remark": "办公室网站", "port": fixture["publicPort"],
                      "bind": "127.0.0.1", "public": True, "enabled": True, "balance": "weighted",
                      "backend": [{"addr": f'127.0.0.1:{fixture["backendPort"]}', "weight": 3}]}
            response = await context.request.post(origin + "/api/batch", data={"operations": [
                {"method": "PUT", "path": "/api/clients/form-device", "value": {
                    "vkey": key, "remark": "办公室电脑", "enabled": True, "allow_config_push": False}},
                {"method": "PUT", "path": "/api/managed/form-device", "value": {
                    "tunnel": [tunnel], "mapping": [], "target_policy": {}}},
            ]}, headers={"Origin": origin, "X-Felucca-CSRF": session["csrf"],
                         "If-Match": f'"{overview["revision"]}"', "Idempotency-Key": secrets.token_hex(16)})
            assert response.ok, await response.text()

            async def choose(locator, label):
                await locator.click()
                await page.get_by_role("option", name=label, exact=True).click()

            async def open_bulk():
                await page.goto(origin + "/#tunnels")
                await choose(page.get_by_role("combobox", name=TEXT["flow.onDevice"]), "办公室电脑")
                group = page.locator("details").filter(has=page.locator("summary").filter(has_text=TEXT["flow.bulkServices"]))
                await group.locator("summary").click()
                await group.get_by_role("button", name=TEXT["action.edit"], exact=True).click()
                await page.locator("#editor[open]").wait_for()

            await open_bulk()
            dialog = page.locator("#editor")
            body = dialog.locator(".dialog-body")
            service = dialog.locator(".config-entry").first
            await expect(service.get_by_role("textbox", name=TEXT["flow.serviceName"], exact=True)).to_be_visible()
            await expect(service.locator('[name="tunnel.0.id"]')).not_to_be_visible()
            enabled = service.get_by_role("switch", name=TEXT["field.enabled"], exact=True)
            await expect(enabled).to_have_count(1)
            await expect(enabled).to_be_checked()
            assert await enabled.evaluate("""e => {
                const rgb = getComputedStyle(e.nextElementSibling).backgroundColor.match(/\\d+/g).map(Number);
                return Math.max(...rgb) < 64 && Math.max(...rgb) - Math.min(...rgb) < 12;
            }"""), "Enabled switch must use the monochrome palette"
            assert await service.locator(":scope > summary .disclosure-status").evaluate("""e => {
                const rgb = getComputedStyle(e).backgroundColor.match(/\\d+/g).map(Number);
                return Math.max(...rgb) < 64 && Math.max(...rgb) - Math.min(...rgb) < 12;
            }"""), "Enabled status must use the monochrome palette"
            assert not await service.get_by_role("checkbox", name=TEXT["field.enabled"], exact=True).count()
            before_switch = len(writes)
            await enabled.focus()
            await page.keyboard.press("Space")
            await expect(enabled).not_to_be_checked()
            await expect(service.locator(":scope > summary .disclosure-status")).to_have_text(TEXT["status.disabled"])
            assert len(writes) == before_switch, "Switch edits must remain a draft until Save"
            await enabled.locator("xpath=ancestor::label").click()
            await expect(enabled).to_be_checked()
            await enabled.focus()
            await page.keyboard.press("Tab")
            await page.keyboard.press("Shift+Tab")
            assert await enabled.evaluate("e => e.matches(':focus-visible')")
            assert await enabled.evaluate("e => getComputedStyle(e.nextElementSibling).outlineStyle !== 'none'")
            summary = service.locator(":scope > summary")
            await summary.focus()
            await page.keyboard.press("Enter")
            await expect(summary).to_have_attribute("aria-expanded", "false")
            await expect(summary).to_contain_text("TCP")
            await expect(summary).to_contain_text(str(fixture["publicPort"]))
            await summary.blur()
            await page.screenshot(animations="disabled", path=str(OUTPUT / "bulk-collapsed.png"))
            await summary.focus()
            await page.keyboard.press("Space")
            await expect(summary).to_have_attribute("aria-expanded", "true")
            await expect(enabled).to_be_checked()
            assert await dialog.locator("details details details").count() == 0
            checks.append("monochrome switches and enabled badges; switch supports keyboard and whole-label activation, exposes on/off state and visible focus, and keeps edits local until Save; keyboard folding retains state and shows current summaries with at most two disclosure levels")
            for bad in ["Public", "Enabled", "Id"]:
                assert not await dialog.get_by_text(bad, exact=True).count()
            await expect(dialog.get_by_role("button", name=TEXT["action.save"], exact=True)).to_be_in_viewport()
            assert await dialog.locator("form").evaluate("e => e.checkValidity()"), "Unchanged valid config must be submittable, including inactive protocols"
            await summary.blur()
            await page.evaluate("document.fonts.ready")
            await page.screenshot(animations="disabled", path=str(OUTPUT / "bulk-desktop.png"))
            await dialog.screenshot(animations="disabled", path=str(OUTPUT / "bulk-preview.png"))
            checks.append("localized, grouped service form; one boolean label; internal ID under advanced settings; persistent save actions")

            name = service.get_by_role("textbox", name=TEXT["flow.serviceName"], exact=True)
            await name.fill("办公室网站（已修改）")
            port = service.locator('[name="tunnel.0.port"]')
            await port.fill("")
            await port.blur()
            count = len(writes)
            await dialog.get_by_role("button", name=TEXT["action.save"], exact=True).click()
            assert len(writes) == count
            assert not await port.evaluate("e => e.validity.valid")
            await port.fill(str(fixture["publicPort"]))
            checks.append("empty numeric input stays visible and blocks submission instead of saving zero or the previous value")

            access = service.locator("summary").filter(has_text=TEXT["form.group.access"]).first.locator("..")
            await access.locator(":scope > summary").click()
            source = service.get_by_role("group", name=TEXT["scope.label.source"], exact=True)
            await source.get_by_role("radio", name=TEXT["scope.source.customLabel"], exact=False).check()
            source_rules = source.locator("..").locator("textarea")
            await expect(source_rules).to_be_visible()
            assert not await source_rules.evaluate("e => e.validity.valid")
            await source.get_by_role("radio", name=TEXT["scope.source.anyLabel"], exact=False).check()

            target = service.get_by_role("group", name=TEXT["scope.label.target"], exact=True)
            await target.get_by_role("radio", name=TEXT["scope.custom"], exact=False).check()
            target_rules = target.locator("..").locator("textarea")
            assert not await target_rules.evaluate("e => e.validity.valid")
            await target_rules.fill("127.0.0.1")
            exception = service.get_by_role("region", name=TEXT["form.destinationExceptions"], exact=True)
            ports = exception.get_by_role("textbox", name=TEXT["scope.ports"], exact=False)
            await ports.fill("70000")
            await access.locator(":scope > summary").click()
            count = len(writes)
            await dialog.get_by_role("button", name=TEXT["action.save"], exact=True).click()
            await expect(ports).to_be_visible()
            await expect(ports).to_be_in_viewport()
            await expect(ports).to_be_focused()
            await page.wait_for_timeout(100)
            assert await ports.evaluate("""e => e.getBoundingClientRect().bottom <=
                e.closest('dialog').querySelector('.dialog-actions').getBoundingClientRect().top"""), "Error input must not be obscured by the sticky actions"
            assert len(writes) == count
            assert not await ports.evaluate("e => e.validity.valid")
            await page.screenshot(animations="disabled", path=str(OUTPUT / "inline-validation.png"))
            await ports.fill(str(fixture["backendPort"]))
            checks.append("empty custom source/destination rules and invalid port lists block saving; hidden invalid fields are revealed")

            await enabled.uncheck()
            async with page.expect_response(lambda r: r.request.method == "PUT" and r.url.endswith("/api/managed/form-device")) as saved:
                await dialog.get_by_role("button", name=TEXT["action.save"], exact=True).click()
            assert (await saved.value).ok
            await dialog.wait_for(state="detached")
            state = await (await context.request.get(origin + "/api/managed/form-device")).json()
            saved_tunnel = state["config"]["tunnel"][0]
            assert saved_tunnel["remark"] == "办公室网站（已修改）"
            assert saved_tunnel["backend"][0]["weight"] == 3
            assert saved_tunnel["balance"] == "weighted" and saved_tunnel["id"] == 1
            assert saved_tunnel["target_policy"]["ports"] == [fixture["backendPort"]]
            assert saved_tunnel["enabled"] is False
            full = await (await context.request.get(origin + "/api/config?secrets=1")).json()
            assert full["config"]["client"]["form-device"]["vkey"] == key
            checks.append("real server save preserves service identity, weighted destinations and credentials while applying the edited name and scope")

            await open_bulk()
            await expect(enabled).not_to_be_checked()
            await enabled.check()
            await expect(page.locator('[name="tunnel.0.remark"]')).to_have_value("办公室网站（已修改）")
            add_backend = TEXT["form.addItem"].replace("{label}", TEXT["form.item.Backend"])
            await service.get_by_role("button", name=add_backend, exact=True).click()
            second_address = service.locator('[name="tunnel.0.backend.1.addr"]')
            await expect(second_address).to_be_visible()
            await second_address.fill("127.0.0.1:9999")
            second_weight = service.locator('[name="tunnel.0.backend.1.weight"]')
            await second_weight.fill("")
            await second_weight.blur()
            second_entry = service.locator("details.config-entry").filter(has=page.locator('[name="tunnel.0.backend.1.addr"]'))
            await second_entry.get_by_role("button", name=TEXT["cap.moveUp"], exact=True).click()
            await expect(service.locator('[name="tunnel.0.backend.0.addr"]')).to_have_value("127.0.0.1:9999")
            await expect(service.locator('[name="tunnel.0.backend.0.weight"]')).to_have_attribute("aria-invalid", "true")
            await expect(service.locator('[name="tunnel.0.backend.1.weight"]')).to_have_value("3")
            await service.locator('[name="tunnel.0.backend.0.weight"]').fill("9")
            first_entry = service.locator("details.config-entry").filter(has=page.locator('[name="tunnel.0.backend.0.addr"]'))
            await first_entry.get_by_role("button", name=TEXT["action.delete"], exact=True).click()
            await expect(service.locator('[name="tunnel.0.backend.0.weight"]')).to_have_value("3")
            checks.append("adding opens the new entry; reordering keeps field errors with their entry; deleting preserves the remaining destination")
            layouts = []
            for width, height, view in [(811, 782, "bulk-compact"), (390, 844, "bulk-mobile"), (844, 390, "bulk-landscape")]:
                await page.set_viewport_size({"width": width, "height": height})
                await body.evaluate("e => e.scrollTop = 0")
                await page.wait_for_timeout(100)
                metrics = await dialog.evaluate("""e => {
                    const b = e.querySelector('.dialog-body'); const r = e.getBoundingClientRect();
                    e.scrollTop = 100;
                    return {outerScroll: e.scrollTop, bodyOverflow: b.scrollWidth-b.clientWidth,
                        pageOverflow: document.documentElement.scrollWidth-innerWidth, top:r.top, bottom:r.bottom};
                }""")
                assert metrics["outerScroll"] <= 1 and metrics["bodyOverflow"] <= 1 and metrics["pageOverflow"] <= 1, metrics
                assert metrics["top"] >= 0 and metrics["bottom"] <= height, metrics
                await expect(dialog.get_by_role("button", name=TEXT["action.save"], exact=True)).to_be_in_viewport()
                await page.screenshot(animations="disabled", path=str(OUTPUT / f"{view}.png"))
                layouts.append({"view":view, **metrics})
            await page.keyboard.press("Escape")
            state = await (await context.request.get(origin + "/api/managed/form-device")).json()
            assert state["config"]["tunnel"][0]["enabled"] is False, "Cancel must discard switch changes"
            checks.append("switch state survives a real save and reload; cancelling a later toggle preserves the saved state")
            await page.set_viewport_size({"width": 1440, "height": 1000})
            await page.get_by_role("button", name=TEXT["flow.addService"], exact=True).first.click()
            wizard = page.locator("#service-wizard")
            await expect(wizard.get_by_text(TEXT["form.routePreview"], exact=True)).to_be_visible()
            await choose(wizard.locator('[name="serviceKind"]').locator('..').get_by_role("combobox"), TEXT["flow.kind.ssh"])
            await expect(wizard.locator('[name="serviceTarget"]')).to_have_value("127.0.0.1:22")
            await page.screenshot(animations="disabled", path=str(OUTPUT / "service-wizard.png"))
            await wizard.screenshot(animations="disabled", path=str(OUTPUT / "service-preview.png"))
            await page.keyboard.press("Escape")
            checks.append("service wizard shows the entry/device/application route and retains working protocol selectors")

            await page.goto(origin + "/#config")
            await page.get_by_role("combobox", name=TEXT["guide.chooseSection"]).wait_for()
            sections = ["bridge", "http", "allowed_ports", "acl_mode", "geo", "ntp", "state", "log", "admin", "node"]
            for section in sections:
                await choose(page.get_by_role("combobox", name=TEXT["guide.chooseSection"]), TEXT[f"guide.server.{section}.title"])
                assert await page.evaluate("document.documentElement.scrollWidth <= innerWidth")
            cert = page.locator("details.surface-disclosure").filter(has=page.locator("summary").get_by_text(TEXT["cap.uploadCertificate"], exact=True))
            await cert.locator("summary").click()
            await choose(cert.get_by_role("combobox", name=TEXT["form.certificateUse"]), TEXT["field.hosts"])
            await expect(cert.get_by_role("textbox", name=TEXT["field.hosts"], exact=True)).to_be_visible()
            await page.screenshot(animations="disabled", path=str(OUTPUT / "certificate-purpose.png"))
            checks.append("all ten server settings sections render; certificate purpose is selected with a conditional domain field")

            for route in ["agent", "deployment", "users", "accounts", "locals", "platforms", "webhooks", "shares", "settings"]:
                await page.goto(origin + "/#" + route)
                await page.locator(".page-content").wait_for()
                await page.wait_for_timeout(300)
                assert not await page.locator(".fatal-error").count()
                assert await page.evaluate("document.documentElement.scrollWidth <= innerWidth")
                if route in ["users", "accounts", "locals", "platforms", "webhooks"]:
                    await page.locator(".page-head").get_by_role("button", name=TEXT["action.add"], exact=True).click()
                    await page.locator("#editor[open]").wait_for()
                    await page.screenshot(animations="disabled", path=str(OUTPUT / f"resource-{route}.png"))
                    await page.keyboard.press("Escape")
                elif route == "agent":
                    await page.screenshot(animations="disabled", path=str(OUTPUT / "agent-settings.png"))
            checks.append("device, access, agent, settings and five resource editor pages render without overflow or browser errors")
            assert not errors, errors
            report = {"checks": checks, "layouts": layouts, "pageErrors": errors,
                      "runtime": "installed Chrome with Vite and a real server" if os.environ.get("FELUCCA_PROBE_VITE") == "1" else "installed Chrome with freshly built embedded production assets"}
            (OUTPUT / "validation.json").write_text(json.dumps(report, ensure_ascii=False, indent=2)+"\n")
            print(json.dumps(report, ensure_ascii=False))
        except Exception:
            await page.screenshot(animations="disabled", path=str(OUTPUT / "failure.png"))
            (OUTPUT / "failure.txt").write_text(await page.locator("body").inner_text())
            raise
        finally:
            await browser.close()


async def main():
    OUTPUT.mkdir(parents=True, exist_ok=True)
    process = await asyncio.create_subprocess_exec(
        "node", "scripts/probes/probe-product-flow.mjs", "--interactive", cwd=ROOT,
        env={**os.environ, "FELUCCA_PROBE_OUTPUT": str(OUTPUT)},
        stdout=asyncio.subprocess.PIPE, stderr=asyncio.subprocess.STDOUT,
    )
    vite = None
    integration = None
    try:
        while True:
            line = await asyncio.wait_for(process.stdout.readline(), timeout=45)
            if not line:
                raise RuntimeError("Fixture exited before becoming ready")
            if b"Empty browser fixture ready" in line:
                break
        fixture = json.loads((OUTPUT / "fixture.json").read_text())
        # Node identity is a startup setting. Use a second isolated listener to
        # exercise platform/callback forms as well as the regular configuration.
        def free_port():
            with socket.socket() as listener:
                listener.bind(("127.0.0.1", 0))
                return listener.getsockname()[1]
        config = json.loads(Path(fixture["serverFile"]).read_text())
        admin_port = free_port()
        config["admin"]["bind"] = f"127.0.0.1:{admin_port}"
        nonce = secrets.token_hex(6)
        config["admin"]["state_path"] = str(OUTPUT / f"forms-runtime-{nonce}.json")
        config["bridge"]["bind"] = f"127.0.0.1:{free_port()}"
        config["p2p"] = None
        config["node"] = {"id": "form-review", "state_path": str(OUTPUT / f"node-{nonce}.json")}
        config_path = OUTPUT / "forms-server.json"
        config_path.write_text(json.dumps(config))
        config_path.chmod(0o600)
        integration = await asyncio.create_subprocess_exec(
            str(ROOT / "target/debug/felucca-server"), "--config", str(config_path), cwd=ROOT,
            env={**os.environ, "FELUCCA_AGENT_ASSETS": str(OUTPUT / "agent-assets")},
            stdout=asyncio.subprocess.DEVNULL, stderr=asyncio.subprocess.DEVNULL,
        )
        fixture["origin"] = f"http://127.0.0.1:{admin_port}"
        for attempt in range(100):
            try:
                response = await asyncio.to_thread(urlopen, fixture["origin"], timeout=1)
                response.close()
                break
            except OSError:
                if integration.returncode is not None:
                    raise RuntimeError("Form server exited during startup")
                await asyncio.sleep(0.1)
        else:
            raise RuntimeError("Form server did not become ready")
        if os.environ.get("FELUCCA_PROBE_VITE") == "1":
            with socket.socket() as listener:
                listener.bind(("127.0.0.1", 0))
                port = listener.getsockname()[1]
            vite = await asyncio.create_subprocess_exec(
                "node", "node_modules/vite/bin/vite.js", "--host", "127.0.0.1", "--port", str(port),
                cwd=ROOT / "apps/admin",
                env={**os.environ, "FELUCCA_ADMIN_TARGET": fixture["origin"]},
                stdout=asyncio.subprocess.PIPE, stderr=asyncio.subprocess.STDOUT,
            )
            while True:
                line = await asyncio.wait_for(vite.stdout.readline(), timeout=30)
                if not line:
                    raise RuntimeError("Vite exited before becoming ready")
                if b"Local:" in line:
                    break
            fixture["origin"] = f"http://127.0.0.1:{port}"
        await exercise(fixture)
    finally:
        if vite and vite.returncode is None:
            vite.terminate()
            await asyncio.wait_for(vite.wait(), timeout=15)
        if integration and integration.returncode is None:
            integration.terminate()
            await asyncio.wait_for(integration.wait(), timeout=15)
        if process.returncode is None:
            process.terminate()
            await asyncio.wait_for(process.wait(), timeout=20)


if __name__ == "__main__":
    asyncio.run(main())
