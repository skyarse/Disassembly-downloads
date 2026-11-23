import QtQuick
import Qt.labs.platform as Platform
import net.skyarse.DownloadsTriage

Item {
    id: host
    property alias backend: worker
    readonly property bool trayAvailable: tray.available
    property string mode: "manual"
    property string root: ""
    property bool configured: false
    property int requestSequence: 0
    property bool confirmOnOpen: false
    readonly property bool lifecycleTest: Qt.application.arguments.includes("--tray-lifecycle-test")
    property int testOpens: 0

    function openQueue() {
        if (!view.item) {
            worker.watching(true)
            view.setSource(Qt.resolvedUrl("Main.qml"), { host: host })
        }
        if (view.item) view.item.showQueue()
    }
    function unloadClosedWindow() {
        if (view.item && !view.item.visible && view.item.canUnload) {
            worker.watching(false)
            view.source = ""
            reclaim.restart()
        }
    }
    Timer {
        id: reclaim
        interval: 200
        onTriggered: {
            if (view.item) return
            gc()
            worker.trimHeap()
            if (host.lifecycleTest) console.log("TRAY_WINDOW_UNLOADED", host.testOpens)
        }
    }
    function toggleMode() {
        if (view.item) {
            if (mode === "automatic") view.item.setMode("paused")
            else {
                openQueue()
                view.item.requestAutomatic()
            }
        } else if (mode === "manual") {
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
    Loader {
        id: view
        onStatusChanged: if (status === Loader.Error) {
            console.error("Failed to load the application window")
            Qt.exit(1)
        }
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
            if (host.lifecycleTest && reply.operation === "tray_tick" && !view.item)
                console.log("TRAY_ONLY_TICK")
            if (view.item && reply.operation !== "tray_tick") {
                view.item.receive(reply)
                if (host.lifecycleTest && reply.operation === "init") {
                    ++host.testOpens
                    if (host.testOpens <= 10) console.log("TRAY_WINDOW_OPEN", host.testOpens)
                    else if (reply.history.some(entry => entry.source.endsWith("/tray-sort.png") && entry.state === "moved")) {
                        console.log("TRAY_REOPENED_WITH_HISTORY")
                        Qt.quit()
                    }
                }
                if (host.confirmOnOpen && reply.operation === "init") {
                    host.confirmOnOpen = false
                    view.item.requestAutomatic()
                }
            }
        }
    }
    Timer { interval: 100; running: true; repeat: true; onTriggered: worker.poll() }
    Platform.SystemTrayIcon {
        id: tray
        visible: available
        icon.source: "qrc:/icons/app.svg"
        tooltip: "Загрузки на разбор · " + (host.mode === "automatic" ? "автоматически" : host.mode === "paused" ? "пауза" : "вручную")
        onActivated: function(reason) {
            if (reason !== Platform.SystemTrayIcon.Trigger && reason !== Platform.SystemTrayIcon.DoubleClick) return
            if (host.lifecycleTest && view.item && host.testOpens <= 10) view.item.close()
            else host.openQueue()
        }
        menu: Platform.Menu {
            Platform.MenuItem { text: "Открыть очередь"; onTriggered: host.openQueue() }
            Platform.MenuItem { text: "Открыть папку загрузок"; enabled: host.configured; onTriggered: Qt.openUrlExternally("file://" + host.root.split("/").map(encodeURIComponent).join("/")) }
            Platform.MenuSeparator {}
            Platform.MenuItem { text: host.mode === "automatic" ? "Приостановить сортировку" : "Продолжить автоматически"; enabled: host.configured && (!view.item || !view.item.busy || host.mode === "automatic"); onTriggered: host.toggleMode() }
            Platform.MenuSeparator {}
            Platform.MenuItem { text: "Завершить работу"; onTriggered: Qt.quit() }
        }
    }
}
