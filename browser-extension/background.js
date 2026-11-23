const api = globalThis.browser ?? globalThis.chrome;
const host = "net.skyarse.downloads_triage";

function webAddress(value) {
    if (typeof value !== "string" || value.length > 16384 || !/^https?:\/\//i.test(value)) {
        return "";
    }
    try {
        const url = new URL(value);
        return url.username || url.password ? "" : url.href;
    } catch {
        return "";
    }
}

async function status(error = "") {
    await api.action.setBadgeText({ text: error ? "!" : "" });
    await api.action.setTitle({ title: error || "Загрузки на разбор" });
}

api.downloads.onChanged.addListener(async delta => {
    if (delta.state?.current !== "complete") return;
    try {
        const [item] = await api.downloads.search({ id: delta.id });
        if (!item || item.incognito || !item.filename?.startsWith("/")) return;

        const file = webAddress(item.url) || webAddress(item.finalUrl);
        const referrer = webAddress(item.referrer);
        const page = referrer === file ? "" : referrer;
        if (page || file) {
            const response = await api.runtime.sendNativeMessage(host, {
                path: item.filename,
                page,
                file,
                size: Number.isSafeInteger(item.fileSize) && item.fileSize >= 0
                    ? item.fileSize
                    : null,
            });
            if (!response?.ok) {
                await status(response?.message || "Источник загрузки не сохранён.");
                return;
            }
            await status();
        }
    } catch {
        await status("Включите интеграцию в настройках «Загрузок на разбор».");
    }
});
