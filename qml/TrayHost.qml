import QtQuick
import Qt.labs.platform as Platform
import net.skyarse.DownloadsTriage

Item {
    id: host
    property string mode: "manual"
    property string root: ""
    property bool configured: false
    property bool windowActive: false
    property bool noTray: false
    property bool windowBusy: false
    property bool confirmOnOpen: false
    readonly property bool lifecycleTest: Qt.application.arguments.includes("--tray-lifecycle-test")
    property int testOpens: 0

    function openQueue() {
        worker.watching(true)
        worker.showWindow(tray.available)
        windowActive = true
        noTray = !tray.available
    }
    function toggleMode() {
        if (worker.windowRunning()) worker.sendWindow('{"window_action":"toggle"}')
        else if (mode === "manual") {
            confirmOnOpen = true
            openQueue()
        } else {
            worker.request(JSON.stringify({op: "set_mode", mode: mode === "paused" ? "automatic" : "paused"}))
        }
    }
    Component.onCompleted: {
        if (Qt.application.arguments.includes("--background") && tray.available)
            worker.request('{"op":"tray_tick"}')
        else openQueue()
    }
    Backend {
        id: worker
        onFinished: {
            let reply = JSON.parse(response)
            if (reply.config) {
                host.mode = reply.config.mode
                host.root = reply.config.root
                host.configured = !reply.first_run
            }
            if (reply.operation === "tray_tick") {
                if (host.lifecycleTest && !worker.windowRunning()) console.log("TRAY_ONLY_TICK")
                return
            }
            if (worker.windowRunning()) {
                worker.sendWindow(response)
                if (reply.request_id != null) host.windowBusy = false
                if (host.confirmOnOpen && reply.operation === "init") {
                    host.confirmOnOpen = false
                    worker.sendWindow('{"window_action":"toggle"}')
                }
                if (host.lifecycleTest && reply.operation === "init") {
                    ++host.testOpens
                    console.log("TRAY_WINDOW_OPEN", host.testOpens)
                    if (host.testOpens > 10 && reply.history.some(entry => entry.source.endsWith("/tray-sort.png") && entry.state === "moved")) {
                        console.log("TRAY_REOPENED_WITH_HISTORY")
                        Qt.quit()
                    }
                }
            }
        }
    }
    Timer {
        interval: 50
        running: true
        repeat: true
        onTriggered: {
            let command
            while (host.windowActive && (command = worker.readWindow()).length) {
                if (JSON.parse(command).request_id != null) host.windowBusy = true
                worker.request(command)
            }
            worker.poll()
            if (host.windowActive && !worker.windowRunning()) {
                host.windowActive = false
                host.windowBusy = false
                worker.watching(false)
                if (host.lifecycleTest) console.log("TRAY_WINDOW_UNLOADED", host.testOpens)
                if (host.noTray || (Qt.application.arguments.includes("--smoke-test") && !host.lifecycleTest))
                    Qt.quit()
            }
        }
    }
    Platform.SystemTrayIcon {
        id: tray
        visible: available
        icon.source: "qrc:/icons/app.svg"
        tooltip: "Загрузки на разбор · " + (host.mode === "automatic" ? "автоматически" : host.mode === "paused" ? "пауза" : "вручную")
        onActivated: function(reason) {
            if (reason !== Platform.SystemTrayIcon.Trigger && reason !== Platform.SystemTrayIcon.DoubleClick) return
            if (host.lifecycleTest && worker.windowRunning() && host.testOpens <= 10) worker.sendWindow('{"window_action":"close"}')
            else host.openQueue()
        }
        menu: Platform.Menu {
            Platform.MenuItem { text: "Открыть очередь"; onTriggered: host.openQueue() }
            Platform.MenuItem { text: "Открыть папку загрузок"; enabled: host.configured; onTriggered: Qt.openUrlExternally("file://" + host.root.split("/").map(encodeURIComponent).join("/")) }
            Platform.MenuSeparator {}
            Platform.MenuItem { text: host.mode === "automatic" ? "Приостановить сортировку" : "Продолжить автоматически"; enabled: host.configured && (!host.windowBusy || host.mode === "automatic"); onTriggered: host.toggleMode() }
            Platform.MenuSeparator {}
            Platform.MenuItem { text: "Завершить работу"; onTriggered: Qt.quit() }
        }
    }
}
