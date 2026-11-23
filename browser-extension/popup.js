const host = "net.skyarse.downloads_triage";
const summary = document.getElementById("summary");
const message = document.getElementById("message");
const list = document.getElementById("files");

function showMessage(text = "") {
    message.textContent = text;
    message.hidden = !text;
}

function webAddress(value) {
    try {
        const url = new URL(value);
        return ["http:", "https:"].includes(url.protocol) && !url.username && !url.password
            ? url.href
            : "";
    } catch {
        return "";
    }
}

async function request(input) {
    const reply = await browser.runtime.sendNativeMessage(host, input);
    if (!reply?.ok) throw new Error(reply?.message);
    return reply;
}

function markUnavailable(button) {
    const label = document.createElement("span");
    label.className = "unavailable";
    label.textContent = "Файл недоступен";
    button.replaceWith(label);
}

function render(page, files) {
    summary.textContent = files.length ? "Уже скачано" : "С этой страницы загрузок пока нет";
    list.hidden = !files.length;
    for (const file of files) {
        const row = document.createElement("li");
        const name = document.createElement("span");
        name.className = "file-name";
        name.textContent = file.name;
        row.append(name);

        const button = document.createElement("button");
        button.type = "button";
        button.textContent = "Показать в Dolphin";
        button.setAttribute("aria-label", "Показать в Dolphin: " + file.name);
        row.append(button);
        if (!file.available) {
            markUnavailable(button);
        } else {
            button.addEventListener("click", async () => {
                button.disabled = true;
                showMessage();
                try {
                    const result = await request({ action: "show_download", page, id: file.id });
                    if (!result.available) markUnavailable(button);
                } catch (error) {
                    showMessage(error.message || "Не удалось связаться с «Загрузками на разбор».");
                } finally {
                    button.disabled = false;
                }
            });
        }
        list.append(row);
    }
}

async function load(page) {
    const query = { action: "list_downloads", page };
    let result = await request(query);
    if (result.legacy_urls.length) {
        const wanted = new Set(result.legacy_urls);
        const downloads = await browser.downloads.search({ state: "complete" });
        const candidates = [];
        for (const item of downloads) {
            if (item.incognito || !item.filename?.startsWith("/")) continue;
            const url = webAddress(item.url) || webAddress(item.finalUrl);
            if (wanted.has(url)) candidates.push({ path: item.filename, url });
            if (wanted.has("") && webAddress(item.referrer) === page) {
                candidates.push({ path: item.filename, url: "" });
            }
        }
        const encode = new TextEncoder();
        const budget = 250000 - encode.encode(JSON.stringify(query)).length;
        let batch = [];
        let length = 0;
        for (const candidate of candidates) {
            const size = encode.encode(JSON.stringify(candidate)).length + 1;
            if (size > budget) continue;
            if (length + size > budget) {
                result = await request({ ...query, candidates: batch });
                batch = [];
                length = 0;
            }
            batch.push(candidate);
            length += size;
        }
        if (batch.length) result = await request({ ...query, candidates: batch });
    }
    render(page, result.files);
}

async function start() {
    try {
        const [tab] = await browser.tabs.query({ active: true, currentWindow: true });
        const page = webAddress(tab?.url);
        if (!page) {
            summary.textContent = "Откройте веб-страницу";
            return;
        }
        await load(page);
    } catch {
        summary.textContent = "Не удалось связаться с «Загрузками на разбор»";
        showMessage("Проверьте связь с браузером в настройках приложения.");
    }
}

start();
