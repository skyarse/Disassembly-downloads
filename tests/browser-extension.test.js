const assert = require("node:assert/strict");
const { createHash } = require("node:crypto");
const { readFileSync } = require("node:fs");
const { test } = require("node:test");
const { runInNewContext } = require("node:vm");

const background = readFileSync("browser-extension/background.js", "utf8");

function start(browser, item, sendNativeMessage) {
    let handler;
    const badges = [];
    const api = {
        downloads: {
            onChanged: { addListener: callback => { handler = callback; } },
            search: async ({ id }) => id === 7 ? [item] : [],
        },
        runtime: { sendNativeMessage },
        action: {
            setBadgeText: async ({ text }) => { badges.push(text); },
            setTitle: async () => {},
        },
    };
    runInNewContext(background, { [browser]: api, URL });
    return { handler, badges };
}

test("sends the original page, download URL and destination only after completion", async () => {
    const sent = [];
    const { handler, badges } = start("browser", {
        filename: "/run/media/user/Drive/Новая папка/f.zip",
        url: "https://files.test/download",
        finalUrl: "https://cdn.test/temporary",
        referrer: "https://site.test/releases/v1",
        fileSize: 27,
    }, async (host, payload) => {
        sent.push([host, JSON.parse(JSON.stringify(payload))]);
        return { ok: true };
    });
    await handler({ id: 7, state: { current: "interrupted" } });
    assert.deepEqual(sent, []);
    await handler({ id: 7, state: { current: "complete" } });
    assert.deepEqual(sent, [["net.skyarse.downloads_triage", {
        path: "/run/media/user/Drive/Новая папка/f.zip",
        page: "https://site.test/releases/v1",
        file: "https://files.test/download",
        size: 27,
    }]]);
    assert.deepEqual(badges, [""]);
});

test("ignores private downloads and reports an unavailable bridge", async () => {
    const item = {
        filename: "/download.bin",
        url: "https://files.test/file.bin",
        incognito: true,
        fileSize: -1,
    };
    let sent = 0;
    const { handler, badges } = start("chrome", item, async (_, payload) => {
        sent++;
        assert.equal(payload.page, "");
        assert.equal(payload.size, null);
        throw new Error("Native host unavailable");
    });
    const complete = { id: 7, state: { current: "complete" } };
    await handler(complete);
    assert.equal(sent, 0);
    item.incognito = false;
    await handler(complete);
    assert.deepEqual(badges, ["!"]);
});

test("browser manifests agree with the native messaging allowlist", () => {
    const firefox = JSON.parse(readFileSync("browser-extension/firefox.json", "utf8"));
    const chromium = JSON.parse(readFileSync("browser-extension/chromium.json", "utf8"));
    const bridge = readFileSync("src/browser_bridge.rs", "utf8");
    const chromiumId = createHash("sha256").update(Buffer.from(chromium.key, "base64"))
        .digest("hex").slice(0, 32).replace(/./g, digit => String.fromCharCode(97 + parseInt(digit, 16)));
    assert.ok(bridge.includes('"' + chromiumId + '"'));
    assert.ok(bridge.includes('"' + firefox.browser_specific_settings.gecko.id + '"'));
    assert.deepEqual(firefox.permissions, ["downloads", "nativeMessaging", "activeTab"]);
    assert.deepEqual(chromium.permissions, ["downloads", "nativeMessaging"]);
});

const popup = readFileSync("browser-extension/popup.js", "utf8");

function element() {
    return {
        children: [],
        append(child) {
            child.replaceWith = replacement => {
                this.children[this.children.indexOf(child)] = replacement;
            };
            this.children.push(child);
        },
        addEventListener(name, listener) { this[name] = listener; },
        setAttribute() {},
    };
}

async function openPopup(url, sendNativeMessage, downloads = []) {
    const elements = { summary: element(), message: element(), files: element() };
    const browser = {
        tabs: { query: async () => [{ url }] },
        runtime: { sendNativeMessage },
        downloads: { search: async () => downloads },
    };
    const document = {
        getElementById: id => elements[id],
        createElement: element,
    };
    await runInNewContext(popup, { URL, TextEncoder, browser, document });
    return elements;
}

test("popup uses the exact page and shows each download with its own status", async () => {
    const page = "https://site.test/version?build=2#stable";
    const requests = [];
    const ui = await openPopup(page, async (host, request) => {
        assert.equal(host, "net.skyarse.downloads_triage");
        requests.push({ ...request });
        if (request.action === "show_download") return { ok: true, available: false };
        return {
            ok: true,
            legacy_urls: [],
            files: [
                { id: 1, name: "available.zip", available: true },
                { id: 2, name: "missing.zip", available: false },
            ],
        };
    });
    assert.equal(ui.summary.textContent, "Уже скачано");
    assert.equal(ui.files.children.length, 2);
    const [first, second] = ui.files.children;
    assert.equal(second.children[1].textContent, "Файл недоступен");
    assert.equal(first.children[1].textContent, "Показать в Dolphin");
    await first.children[1].click();
    assert.equal(first.children[1].textContent, "Файл недоступен");
    assert.deepEqual(requests, [
        { action: "list_downloads", page },
        { action: "show_download", page, id: 1 },
    ]);
});

test("popup uses Firefox history for legacy paths and separates host failures", async () => {
    const page = "https://site.test/legacy";
    const direct = "https://files.test/file.zip";
    const ui = await openPopup(page, async (_, request) => {
        if (!request.candidates) return { ok: true, files: [], legacy_urls: [direct] };
        assert.deepEqual(JSON.parse(JSON.stringify(request.candidates)), [
            { path: "/mnt/old/file.zip", url: direct },
        ]);
        return { ok: true, files: [{ id: 7, name: "file.zip", available: true }], legacy_urls: [] };
    }, [
        { filename: "/mnt/old/file.zip", url: direct },
        { filename: "/mnt/private/file.zip", url: direct, incognito: true },
        { filename: "/mnt/unrelated/file.zip", url: "https://other.test/file.zip" },
    ]);
    assert.equal(ui.summary.textContent, "Уже скачано");
    assert.equal(ui.files.children[0].children[0].textContent, "file.zip");

    const failed = await openPopup(page, async () => { throw new Error("host missing"); });
    assert.equal(failed.summary.textContent, "Не удалось связаться с «Загрузками на разбор»");
    assert.equal(failed.files.children.length, 0);
});
