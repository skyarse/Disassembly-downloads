import QtQuick
import QtQuick.Controls
import QtQuick.Layouts
import QtQuick.Dialogs
import Qt.labs.folderlistmodel
import Qt.labs.platform as Platform
import net.skyarse.DownloadsTriage

ApplicationWindow {
    id: window
    width: 1180
    height: 800
    minimumWidth: 880
    minimumHeight: 640
    visible: !Qt.application.arguments.includes("--background") || !tray.available
    title: "Загрузки на разбор"
    font.family: "Sans Serif"
    font.pixelSize: 14
    property var appState: ({queue: [], history: [], matches: [], first_run: true})
    property var config: ({root: "", root_automatic: true, sources: [], only_new: false, categories: [], rules: [], exclusions: [], mode: "manual", theme: "system", autostart: false})
    property bool busy: false
    property int requestSequence: 0
    property int pendingRequest: -1
    property string pendingOperation: ""
    property int receivedReplies: 0
    property var selected: ({})
    property var destinations: ({})
    property string folderPurpose: ""
    property string folderToken: ""
    property var moveItems: []
    property var pendingDecision: null
    property bool rootDirty: false
    property bool quitting: false
    property string notice: "Загрузка настроек…"
    property string themeChoice: "system"
    readonly property string mode: config.mode || "manual"
    readonly property bool dark: themeChoice === "dark" || (themeChoice === "system" && window.palette.window.hslLightness < 0.5)
    readonly property color canvasColor: dark ? "#171412" : "#f6f4f0"
    readonly property color cardColor: dark ? "#211d1a" : "#ffffff"
    readonly property color sideColor: dark ? "#1c1815" : "#f1ede6"
    readonly property color ink: dark ? "#f5f0ea" : "#29231d"
    readonly property color muted: dark ? "#b1a497" : "#807366"
    readonly property color lineColor: dark ? "#3c332b" : "#e7e0d7"
    readonly property color accent: dark ? "#fbbf24" : "#f4a20b"
    readonly property color accentWash: dark ? "#46331b" : "#fff0cf"
    readonly property color danger: dark ? "#ff9c8f" : "#b6483c"
    readonly property color success: dark ? "#80c79f" : "#3c8060"
    readonly property var pageTitles: ["Очередь на разбор", "Правила сортировки", "Категории", "История перемещений", "Настройки"]
    readonly property var pageHints: ["Только то, для чего нужно ваше решение", "Проверяйте условия и задавайте порядок", "Выбирайте, куда будут попадать файлы", "Каждое перемещение можно проверить и отменить", "Папка загрузок, режим работы и внешний вид"]
    readonly property var additionalSources: config.sources || []
    readonly property var sourcePaths: [config.root].concat(additionalSources.map(source => source.path))
    readonly property bool hasError: !!appState.error || !!(appState.source_errors || []).length
    readonly property var queueRows: (appState.queue || []).filter(row => mode !== "automatic" || !isAutomaticCandidate(row))
    readonly property int waitingCount: mode === "automatic" ? automaticCandidates.length : 0
    readonly property var automaticCandidates: (appState.queue || []).filter(isAutomaticCandidate)

    function isAutomaticCandidate(row) { return row.automatic && row.action === "move" && !row.manual_only }
    function relativeFolder(root, folder) {
        let prefix = root.endsWith("/") ? root : root + "/"
        return folder.startsWith(prefix) ? folder.slice(prefix.length) : folder
    }
    function destinationPath(root, folder) {
        return folder.startsWith("/") ? folder : (root.endsWith("/") ? root : root + "/") + folder
    }
    function updateSource(index, key, value) {
        let edited = copy(config)
        if (index < 0) edited[key] = value
        else edited.sources[index][key] = value
        saveConfig(edited)
    }

    QtObject { id: tabs; property int currentIndex: 0 }
    function send(command) {
        if (busy) return false
        command.request_id = ++requestSequence
        pendingRequest = command.request_id
        pendingOperation = command.op
        busy = true
        backend.request(JSON.stringify(command))
        return true
    }
    function showQueue() { tabs.currentIndex = 0; window.show(); window.raise(); window.requestActivate() }
    function setMode(value) {
        if (busy && value !== "automatic") {
            let command = {op: "set_mode", mode: value, request_id: ++requestSequence}
            pendingRequest = command.request_id
            pendingOperation = command.op
            backend.request(JSON.stringify(command))
        } else send({op: "set_mode", mode: value})
    }
    function requestAutomatic() {
        if (mode === "paused") setMode("automatic")
        else autoConfirm.open()
    }
    function setTheme(value) {
        themeChoice = value
        send({op: "set_theme", theme: value})
    }
    function basename(path) { return String(path || "").split("/").filter(part => part.length).pop() || "Папка не выбрана" }
    function sizeLabel(size) { return size < 1048576 ? Math.max(1, Math.round(size / 1024)) + " КБ" : (size / 1048576).toFixed(1) + " МБ" }
    function keepValid(values, rows) {
        let result = {}, ids = rows.map(row => row.id)
        Object.keys(values).forEach(id => { if (ids.includes(id)) result[id] = values[id] })
        return result
    }
    function copy(value) { return JSON.parse(JSON.stringify(value)) }
    function saveConfig(value) { value.theme = themeChoice; send({op: "save", config: value}) }
    function localPath(url) { return decodeURIComponent(url.toString().replace(/^file:\/\//, "")) }
    function folderUrl(path) { return "file://" + path.split("/").map(encodeURIComponent).join("/") }
    function target(row) { return destinations[row.id] !== undefined ? destinations[row.id] : row.destination }
    function select(row, checked) {
        let value = copy(selected)
        if (checked) value[row.id] = true
        else delete value[row.id]
        selected = value
    }
    function chooseFolder(purpose, token, root) {
        folderPurpose = purpose
        folderToken = token || ""
        folders.currentFolder = folderUrl(root || config.root || "/")
        folders.open()
    }
    function confirmMove(rows) {
        moveItems = rows.filter(row => row.ready).map(row => ({id: row.id, folder: target(row), name: row.name, source: row.source}))
        if (!moveItems.length) { notice = "Выберите готовые файлы."; return }
        if (moveItems.some(row => !row.folder)) { notice = "Укажите папку назначения для каждого выбранного файла."; return }
        moveConfirm.open()
    }
    function editRule(index, row) {
        ruleDialog.ruleIndex = index
        ruleDialog.tested = false
        ruleDialog.matchesText = ""
        ruleName.text = row ? row.name : ""
        extensions.text = row ? row.condition.extensions.join(", ") : ""
        pattern.text = row ? row.condition.pattern : ""
        ruleCategory.currentIndex = row ? Math.max(0, config.categories.findIndex(c => c.id === row.category_id)) : 0
        ruleAction.currentIndex = row && row.action === "suggest" ? 1 : 0
        ruleActive.checked = row ? row.active : true
        let paths = [""].concat(sourcePaths.filter(path => path.length))
        let scope = row && row.source ? row.source : ""
        if (!paths.includes(scope)) paths.push(scope)
        ruleDialog.scopePaths = paths
        ruleScope.currentIndex = paths.indexOf(scope)
        ruleDialog.ruleId = row ? row.id : "rule-" + Date.now()
        ruleDialog.open()
    }
    function conditionFromEditor() {
        return {extensions: extensions.text.split(/[,;\s]+/).map(x => x.replace(/^\./, "").toLowerCase()).filter(x => x.length), pattern: pattern.text}
    }
    function ruleFromDecision(row) {
        let folder = target(row)
        let source = row.source || config.root
        let relative = relativeFolder(source, folder)
        let category = config.categories.find(c => c.folder === folder || c.folder === relative)
        if (!category) {
            pendingDecision = {name: row.name, destination: relative, source: source}
            categoryDialog.categoryIndex = -1
            categoryDialog.categoryId = "category-" + Date.now()
            categoryName.text = relative.split("/").filter(x => x.length).pop() || ""
            categoryFolder.text = relative
            categoryDialog.open()
            return
        }
        editRule(-1, null)
        ruleName.text = row.name
        pattern.text = row.name.replace(/([*?[\]{}\\])/g, "\\$1")
        ruleCategory.currentIndex = config.categories.findIndex(c => c.id === category.id)
        if (additionalSources.length) ruleScope.currentIndex = ruleDialog.scopePaths.indexOf(source)
    }
    function stateName(value) {
        return ({moved: "Перемещено", undone: "Отменено", planned: "Начато", undo_planned: "Отмена начата", needs_review: "Нужна проверка", error: "Не завершено"})[value] || value
    }


    component Copy: Label {
        color: window.ink
        textFormat: Text.PlainText
        font.pixelSize: 14
        Layout.minimumWidth: 0
    }
    component Small: Copy { color: window.muted; font.pixelSize: 12; wrapMode: Text.Wrap }
    component Heading: Copy { font.pixelSize: 16; font.bold: true }
    component ActionButton: Button {
        id: control
        property bool primary: false
        property bool destructive: false
        property bool compact: false
        implicitHeight: 40
        implicitWidth: Math.max(compact ? 40 : 96, contentItem.implicitWidth + 32)
        leftPadding: 16
        rightPadding: 16
        opacity: enabled ? 1 : 0.45
        font.pixelSize: 14
        activeFocusOnTab: true
        contentItem: Copy {
            text: control.text
            color: control.primary ? "#302111" : (control.destructive ? window.danger : window.ink)
            font.pixelSize: 14
            font.bold: control.primary
            horizontalAlignment: Text.AlignHCenter
            verticalAlignment: Text.AlignVCenter
        }
        background: Rectangle {
            radius: 8
            color: control.primary ? (control.down ? "#dd9005" : window.accent) : (control.hovered || control.down ? window.sideColor : window.cardColor)
            border.color: control.activeFocus ? window.accent : (control.primary ? "transparent" : window.lineColor)
            border.width: control.activeFocus ? 2 : 1
        }
    }
    component Input: TextField {
        id: control
        implicitHeight: 42
        implicitWidth: 220
        leftPadding: 12
        rightPadding: 12
        color: window.ink
        placeholderTextColor: window.muted
        selectionColor: window.accent
        selectedTextColor: "#302111"
        font.pixelSize: 14
        opacity: enabled ? 1 : 0.55
        background: Rectangle {
            radius: 8
            color: control.enabled ? window.cardColor : window.canvasColor
            border.color: control.activeFocus ? window.accent : window.lineColor
            border.width: control.activeFocus ? 2 : 1
        }
    }
    component Memo: TextArea {
        id: control
        padding: 12
        color: window.ink
        placeholderTextColor: window.muted
        selectionColor: window.accent
        selectedTextColor: "#302111"
        wrapMode: TextEdit.Wrap
        textFormat: TextEdit.PlainText
        font.pixelSize: 13
        background: Rectangle { radius: 8; color: window.canvasColor; border.color: control.activeFocus ? window.accent : window.lineColor }
    }
    component Select: ComboBox {
        id: control
        implicitHeight: 42
        implicitWidth: 210
        leftPadding: 12
        rightPadding: 32
        opacity: enabled ? 1 : 0.5
        contentItem: Copy { text: control.displayText; elide: Text.ElideRight; verticalAlignment: Text.AlignVCenter }
        indicator: Copy { text: "⌄"; color: window.muted; font.pixelSize: 18; x: control.width - 25; y: (control.height - height) / 2 }
        background: Rectangle { radius: 8; color: window.cardColor; border.color: control.activeFocus ? window.accent : window.lineColor; border.width: control.activeFocus ? 2 : 1 }
        delegate: Component { ItemDelegate {
            id: option
            required property var modelData
            required property int index
            width: control.width - 12
            height: 38
            text: control.textRole ? modelData[control.textRole] : String(modelData)
            highlighted: control.highlightedIndex === index
            contentItem: Copy { text: option.text; elide: Text.ElideRight; verticalAlignment: Text.AlignVCenter }
            background: Rectangle { radius: 6; color: option.highlighted || option.hovered ? window.accentWash : "transparent" }
        } }
        popup: Popup {
            y: control.height + 5
            width: control.width
            padding: 6
            implicitHeight: Math.min(contentItem.implicitHeight + 12, 280)
            background: Rectangle { radius: 10; color: window.cardColor; border.color: window.lineColor }
            contentItem: ListView {
                clip: true
                implicitHeight: contentHeight
                model: control.popup.visible ? control.delegateModel : null
                currentIndex: control.highlightedIndex
                ScrollIndicator.vertical: ScrollIndicator {}
            }
        }
    }
    component Check: CheckBox {
        id: control
        implicitHeight: 32
        implicitWidth: text.length ? contentItem.implicitWidth + 30 : 22
        spacing: 8
        opacity: enabled ? 1 : 0.5
        indicator: Rectangle {
            x: 0
            y: (control.height - height) / 2
            width: 19
            height: 19
            radius: 5
            color: control.checked ? window.accent : window.cardColor
            border.color: control.activeFocus ? window.accent : window.lineColor
            border.width: control.activeFocus ? 2 : 1
            Copy { anchors.centerIn: parent; text: "✓"; visible: control.checked; color: "#302111"; font.pixelSize: 14; font.bold: true }
        }
        contentItem: Copy { text: control.text; leftPadding: 28; verticalAlignment: Text.AlignVCenter }
    }
    component Card: Control {
        id: control
        padding: 20
        implicitHeight: column.implicitHeight + topPadding + bottomPadding
        default property alias body: column.data
        contentItem: ColumnLayout { id: column; spacing: 14 }
        background: Rectangle { radius: 13; color: window.cardColor; border.color: window.lineColor }
    }
    component Sheet: Dialog {
        id: control
        default property alias body: sheetBody.data
        parent: Overlay.overlay
        anchors.centerIn: parent
        modal: true
        width: Math.min(window.width - 64, 650)
        padding: 24
        closePolicy: Popup.CloseOnEscape
        background: Rectangle { radius: 16; color: window.cardColor; border.color: window.lineColor }
        header: Heading { text: control.title; font.pixelSize: 21; padding: 24; bottomPadding: 8 }
        contentItem: ScrollView {
            id: sheetScroll
            implicitWidth: 0
            clip: true
            contentWidth: availableWidth
            implicitHeight: Math.min(sheetBody.implicitHeight, window.height - control.header.implicitHeight - control.topPadding - control.bottomPadding - 32)
            Column { id: sheetBody; width: sheetScroll.availableWidth }
        }
        Overlay.modal: Rectangle { color: window.dark ? "#aa000000" : "#663b3124" }
    }
    component Tag: Rectangle {
        property alias text: label.text
        property color textColor: window.muted
        property color fill: window.sideColor
        implicitWidth: label.implicitWidth + 20
        implicitHeight: 28
        radius: 14
        color: fill
        Copy { id: label; anchors.centerIn: parent; color: parent.textColor; font.pixelSize: 12 }
    }
    component LineIcon: Canvas {
        id: icon
        property string symbol: "folder"
        property color ink: window.muted
        implicitWidth: 22
        implicitHeight: 22
        onSymbolChanged: requestPaint()
        onInkChanged: requestPaint()
        onPaint: {
            let c = getContext("2d")
            c.reset(); c.scale(width / 24, height / 24); c.strokeStyle = ink; c.lineWidth = 1.7; c.lineJoin = "round"; c.lineCap = "round"
            let lines = {
                folder: [[3,6,10,6,12,9,21,9,21,20,3,20,3,6]],
                queue: [[4,4,20,4,20,20,4,20,4,4],[8,9,16,9],[8,14,13,14]],
                rules: [[9,5,20,5],[9,12,20,12],[9,19,20,19],[3,5,4,6,6,3],[3,12,4,13,6,10],[3,19,4,20,6,17]],
                history: [[3,5,3,10,8,10],[12,7,12,13,16,15]],
                settings: [[4,6,20,6],[4,12,20,12],[4,18,20,18],[9,3,9,9],[16,9,16,15],[8,15,8,21]],
                download: [[12,3,12,15],[7,10,12,15,17,10],[4,17,4,21,20,21,20,17]]
            }
            if (symbol === "history") { c.beginPath(); c.arc(12,12,9,-2.5,2.5); c.stroke() }
            ;(lines[symbol] || lines.folder).forEach(path => { c.beginPath();c.moveTo(path[0],path[1]);for(let i=2;i<path.length;i+=2)c.lineTo(path[i],path[i+1]);c.stroke() })
        }
    }

    Backend {
        id: backend
        onFinished: window.receive(backend.response)
    }
    function receive(value) {
        let response
        try { response = JSON.parse(value) } catch (error) { busy = false; notice = "Не удалось прочитать ответ: " + error; return }
        receivedReplies += 1
        let wasFirstRun = !!appState.first_run
        let hadError = hasError
        let requested = response.request_id === pendingRequest
        if (requested) { busy = false; pendingOperation = "" }
        if (response.config) {
            window.appState = response
            window.config = response.config
            if (!rootDirty || response.operation === "save") { rootField.text = response.config.root; rootDirty = false }
            themeChoice = response.config.theme || "system"
            selected = keepValid(selected, response.queue)
            destinations = keepValid(destinations, response.queue)
        }
        if (response.error) notice = response.error
        else if (response.source_errors && response.source_errors.length) notice = "Не удалось проверить: " + response.source_errors.join("; ")
        else if (response.message) notice = response.message
        else if (requested || hadError) notice = response.first_run ? "Выберите папку, чтобы начать. Сначала покажем файлы, ничего не перемещая." : "Все изменения сохраняются локально."
        if (ruleDialog.visible && ruleDialog.awaitingTest && requested && response.operation === "test") {
            ruleDialog.awaitingTest = false
            ruleDialog.tested = !response.error
            ruleDialog.matchesText = response.error || (response.matches.length ? response.matches.join("\n") : "Совпадений нет. Попробуйте изменить условие.")
        }
        if (response.first_run) tabs.currentIndex = 4
        else if (wasFirstRun) tabs.currentIndex = 0
        if (pendingDecision && !categoryDialog.visible && !response.error && requested) {
            let decision = pendingDecision; pendingDecision = null; ruleFromDecision(decision)
        }
    }
    Component.onCompleted: {
        let args = Qt.application.arguments
        let widthIndex = args.indexOf("--test-width")
        if (widthIndex >= 0) window.width = Number(args[widthIndex + 1])
        let heightIndex = args.indexOf("--test-height")
        if (heightIndex >= 0) window.height = Number(args[heightIndex + 1])
        send({op: "init"})
    }
    Timer { interval: 100; running: true; repeat: true; onTriggered: backend.poll() }
    onClosing: function(close) {
        if (tray.available && !quitting) { close.accepted = false; window.hide() }
    }
    Platform.SystemTrayIcon {
        id: tray
        visible: available
        icon.source: "qrc:/icons/app.svg"
        tooltip: "Загрузки на разбор · " + (mode === "automatic" ? "автоматически" : mode === "paused" ? "пауза" : "вручную")
        onActivated: function(reason) { if (reason === Platform.SystemTrayIcon.Trigger || reason === Platform.SystemTrayIcon.DoubleClick) showQueue() }
        menu: Platform.Menu {
            Platform.MenuItem { text: "Открыть очередь"; onTriggered: showQueue() }
            Platform.MenuItem { text: "Открыть папку загрузок"; enabled: !appState.first_run; onTriggered: Qt.openUrlExternally(folderUrl(config.root)) }
            Platform.MenuSeparator {}
            Platform.MenuItem { text: mode === "automatic" ? "Приостановить сортировку" : "Продолжить автоматически"; enabled: !appState.first_run && (!busy || mode === "automatic"); onTriggered: mode === "automatic" ? setMode("paused") : requestAutomatic() }
            Platform.MenuSeparator {}
            Platform.MenuItem { text: "Завершить работу"; onTriggered: { quitting = true; Qt.quit() } }
        }
    }
    FolderDialog {
        id: folders
        title: folderPurpose === "source" ? "Добавьте папку для наблюдения" : folderPurpose === "root" ? "Выберите папку загрузок" : "Выберите подпапку назначения"
        onAccepted: {
            let path = localPath(selectedFolder)
            if (folderPurpose === "root") { rootField.text = path; rootDirty = true }
            else if (folderPurpose === "source") {
                let value = copy(config)
                value.sources.push({path: path, automatic: false, only_new: false})
                saveConfig(value)
            }
            else if (folderPurpose === "category") categoryFolder.text = path
            else { let value = copy(destinations); value[folderToken] = path; destinations = value }
        }
    }
    FileDialog { id: importFile; title: "Импорт настроек"; nameFilters: ["Настройки JSON (*.json)"]; onAccepted: importConfirm.open() }
    FileDialog { id: exportFile; title: "Экспорт настроек"; fileMode: FileDialog.SaveFile; nameFilters: ["Настройки JSON (*.json)"]; defaultSuffix: "json"; onAccepted: send({op: "export", path: localPath(selectedFile)}) }
    Sheet {
        id: importConfirm
        title: "Импортировать настройки?"
        ColumnLayout {
            width: parent.width
            spacing: 20
            Copy { text: "Правила и категории будут заменены после проверки файла. Сортировка переключится в ручной режим — сначала можно будет проверить результат."; wrapMode: Text.Wrap; Layout.fillWidth: true }
            RowLayout { Layout.alignment: Qt.AlignRight; ActionButton { text: "Отмена"; onClicked: importConfirm.close() } ActionButton { text: "Импортировать"; primary: true; onClicked: { importConfirm.close(); send({op: "import", path: localPath(importFile.selectedFile)}) } } }
        }
    }
    Sheet {
        id: moveConfirm
        title: "Подтвердить перемещение"
        ColumnLayout {
            width: parent.width
            spacing: 18
            Copy { text: "Будут перемещены выбранные файлы: " + moveItems.length; Layout.fillWidth: true }
            ScrollView { implicitWidth: 0; contentWidth: availableWidth; Layout.fillWidth: true; Layout.preferredHeight: Math.min(220, confirmationText.implicitHeight + 20); clip: true; Memo { id: confirmationText; readOnly: true; width: parent.width; text: moveItems.map(row => row.name + "\n→ " + destinationPath(row.source, row.folder)).join("\n\n") } }
            RowLayout { Layout.alignment: Qt.AlignRight; ActionButton { text: "Отмена"; onClicked: moveConfirm.close() } ActionButton { text: "Переместить"; primary: true; onClicked: { moveConfirm.close(); send({op: "move", items: moveItems}) } } }
        }
    }
    Sheet {
        id: autoConfirm
        title: "Включить автоматическую сортировку?"
        ColumnLayout {
            width: parent.width
            spacing: 18
            Copy { text: "Правила с действием «Переместить» начнут работать сами. Файлы без правила и с действием «Предложить» останутся на разбор."; wrapMode: Text.Wrap; Layout.fillWidth: true }
            Tag { text: "Подходящих файлов сейчас: " + automaticCandidates.length; fill: window.accentWash; textColor: window.ink }
            ScrollView { implicitWidth: 0; contentWidth: availableWidth; Layout.fillWidth: true; Layout.preferredHeight: Math.min(180, autoPreview.implicitHeight + 20); visible: automaticCandidates.length > 0; clip: true; Memo { id: autoPreview; readOnly: true; width: parent.width; text: automaticCandidates.map(row => row.name + " → " + destinationPath(row.source, row.destination)).join("\n") } }
            Small { text: "Перемещение начнётся только после минуты без изменений. Отменённые и отложенные файлы автоматически не обрабатываются."; Layout.fillWidth: true }
            RowLayout { Layout.alignment: Qt.AlignRight; ActionButton { text: "Пока вручную"; onClicked: autoConfirm.close() } ActionButton { text: "Включить авто"; primary: true; onClicked: { autoConfirm.close(); setMode("automatic") } } }
        }
    }
    Sheet {
        id: categoryDialog
        property int categoryIndex: -1
        property string categoryId: ""
        title: categoryIndex < 0 ? "Новая категория" : "Изменить категорию"
        onRejected: pendingDecision = null
        ColumnLayout {
            width: parent.width
            spacing: 12
            Copy { text: "Название" }
            Input { id: categoryName; Layout.fillWidth: true; placeholderText: "Например, Скриншоты" }
            Copy { text: "Папка назначения" }
            RowLayout { Layout.fillWidth: true; Input { id: categoryFolder; Layout.fillWidth: true; Layout.minimumWidth: 180; placeholderText: "Картинки/Скриншоты" } ActionButton { text: "Выбрать…"; onClicked: chooseFolder("category", "") } }
            Small { text: "Можно указать новую подпапку внутри загрузок — она создастся при первом переносе."; Layout.fillWidth: true }
            RowLayout {
                Layout.alignment: Qt.AlignRight
                Layout.topMargin: 10
                ActionButton { text: "Отмена"; onClicked: { pendingDecision = null; categoryDialog.close() } }
                ActionButton {
                    text: "Сохранить категорию"; primary: true; enabled: !busy && categoryName.text.trim().length > 0 && categoryFolder.text.trim().length > 0
                    onClicked: {
                        let value = copy(config), item = {id: categoryDialog.categoryId, name: categoryName.text.trim(), folder: categoryFolder.text.trim()}
                        if (categoryDialog.categoryIndex < 0) value.categories.push(item)
                        else value.categories[categoryDialog.categoryIndex] = item
                        categoryDialog.close(); saveConfig(value)
                    }
                }
            }
        }
    }
    Sheet {
        id: ruleDialog
        property int ruleIndex: -1
        property string ruleId: ""
        property bool tested: false
        property bool awaitingTest: false
        property string matchesText: ""
        property var scopePaths: [""]
        title: ruleIndex < 0 ? "Новое правило" : "Изменить правило"
        width: Math.min(window.width - 64, 700)
        ColumnLayout {
            width: parent.width
            spacing: 10
            Copy { text: "Название правила" }
            Input { id: ruleName; enabled: !busy; Layout.fillWidth: true; placeholderText: "Например, Скриншоты в отдельную папку"; onTextEdited: ruleDialog.tested = false }
            RowLayout {
                Layout.fillWidth: true
                spacing: 16
                ColumnLayout { Layout.fillWidth: true; Copy { text: "Расширения" } Input { id: extensions; enabled: !busy; Layout.fillWidth: true; placeholderText: "png, jpg"; onTextEdited: ruleDialog.tested = false } }
                ColumnLayout { Layout.fillWidth: true; Copy { text: "Шаблон имени" } Input { id: pattern; enabled: !busy; Layout.fillWidth: true; placeholderText: "Screenshot*"; onTextEdited: ruleDialog.tested = false } }
            }
            Small { text: "Пустое поле — любое значение. Если заполнены оба, должны совпасть оба условия. * заменяет любой текст, ? — один символ."; Layout.fillWidth: true }
            RowLayout {
                Layout.fillWidth: true
                spacing: 16
                ColumnLayout { Layout.fillWidth: true; Copy { text: "Куда переместить" } Select { id: ruleCategory; enabled: !busy; Layout.fillWidth: true; model: config.categories; textRole: "name"; onActivated: ruleDialog.tested = false } }
                ColumnLayout { Layout.fillWidth: true; Copy { text: "Действие" } Select { id: ruleAction; enabled: !busy; Layout.fillWidth: true; model: ["Переместить", "Предложить"] } }
            }
            Check { id: ruleActive; text: "Правило активно"; enabled: !busy }
            RowLayout {
                Layout.fillWidth: true
                visible: additionalSources.length > 0 || ruleScope.currentIndex > 0
                Copy { text: "Применять к папке" }
                Select {
                    id: ruleScope
                    Layout.fillWidth: true
                    enabled: !busy
                    model: ruleDialog.scopePaths.map(path => path || "Все папки")
                    onActivated: ruleDialog.tested = false
                }
            }
            Small { text: "Первое совпадение побеждает. Новое правило добавляется в начало. «Предложить» всегда требует подтверждения."; Layout.fillWidth: true }
            RowLayout { Layout.fillWidth: true; ActionButton { text: ruleDialog.awaitingTest ? "Проверяю…" : "Проверить совпадения"; enabled: !busy; onClicked: { ruleDialog.awaitingTest = true; send({op: "test", condition: conditionFromEditor(), source: ruleDialog.scopePaths[ruleScope.currentIndex] || null}) } } Small { text: ruleDialog.tested ? "Условие проверено" : "Проверьте перед сохранением"; color: ruleDialog.tested ? window.success : window.muted; Layout.fillWidth: true } }
            ScrollView { implicitWidth: 0; contentWidth: availableWidth; Layout.fillWidth: true; Layout.preferredHeight: 92; clip: true; Memo { width: parent.width; readOnly: true; text: ruleDialog.matchesText; placeholderText: "Здесь появятся файлы, подходящие под условие." } }
            RowLayout {
                Layout.alignment: Qt.AlignRight
                ActionButton { text: "Отмена"; onClicked: ruleDialog.close() }
                ActionButton {
                    text: "Сохранить правило"; primary: true
                    enabled: ruleDialog.tested && !busy && config.categories.length > 0 && ruleName.text.trim().length > 0
                    onClicked: {
                        let value = copy(config)
                        let rule = {id: ruleDialog.ruleId, name: ruleName.text.trim(), condition: conditionFromEditor(), category_id: config.categories[ruleCategory.currentIndex].id, action: ruleAction.currentIndex === 0 ? "move" : "suggest", active: ruleActive.checked}
                        rule.source = ruleDialog.scopePaths[ruleScope.currentIndex] || null
                        if (ruleDialog.ruleIndex < 0) value.rules.unshift(rule)
                        else value.rules[ruleDialog.ruleIndex] = rule
                        ruleDialog.close(); saveConfig(value)
                    }
                }
            }
        }
    }
    Sheet {
        id: exclusionDialog
        title: "Новое исключение"
        ColumnLayout {
            width: parent.width
            spacing: 12
            Small { text: "Совпавшие файлы останутся на месте и не попадут в очередь. Исключения проверяются первыми."; Layout.fillWidth: true }
            Copy { text: "Расширения" }
            Input { id: exclusionExt; Layout.fillWidth: true; placeholderText: "bak, log" }
            Copy { text: "Шаблон имени" }
            Input { id: exclusionPattern; Layout.fillWidth: true; placeholderText: "keep-*" }
            RowLayout {
                Layout.alignment: Qt.AlignRight
                ActionButton { text: "Отмена"; onClicked: exclusionDialog.close() }
                ActionButton { text: "Добавить исключение"; primary: true; enabled: exclusionExt.text.trim().length > 0 || exclusionPattern.text.trim().length > 0; onClicked: { let value = copy(config); value.exclusions.push({extensions: exclusionExt.text.split(/[,;\s]+/).map(x => x.replace(/^\./, "").toLowerCase()).filter(x => x.length), pattern: exclusionPattern.text}); exclusionDialog.close(); saveConfig(value) } }
            }
        }
    }
    Sheet {
        id: aboutDialog
        title: "О приложении"
        ColumnLayout {
            width: parent.width
            spacing: 16
            Copy { text: "Загрузки на разбор · 0.1.0"; font.bold: true }
            Small { text: "Файлы обрабатываются на вашем компьютере. Данные и история не отправляются в интернет.\n\nGPL-3.0-only. Программа предоставляется без гарантий."; Layout.fillWidth: true }
            RowLayout { Layout.fillWidth: true; ActionButton { text: "Исходный код ↗"; onClicked: Qt.openUrlExternally("https://github.com/skyarse/Disassembly-downloads") } Item { Layout.fillWidth: true } ActionButton { text: "Закрыть"; onClicked: aboutDialog.close() } }
        }
    }

    Item {
        id: page
        anchors.fill: parent
        Rectangle { anchors.fill: parent; color: window.canvasColor }
        RowLayout {
            anchors.fill: parent
            spacing: 0
            Rectangle {
                Layout.preferredWidth: window.width < 1000 ? 204 : 224
                Layout.fillHeight: true
                color: window.sideColor
                Rectangle { anchors.right: parent.right; height: parent.height; width: 1; color: window.lineColor }
                ColumnLayout {
                    anchors.fill: parent
                    anchors.margins: 16
                    spacing: 8
                    RowLayout {
                        Layout.fillWidth: true
                        Layout.topMargin: 12
                        Layout.bottomMargin: 28
                        spacing: 12
                        Rectangle { Layout.preferredWidth: 40; Layout.preferredHeight: 40; radius: 12; color: window.accent; LineIcon { anchors.centerIn: parent; symbol: "download"; ink: "#50320a" } }
                        Copy { text: "Загрузки\nна разбор"; font.pixelSize: 16; font.bold: true; lineHeight: 1.15; Layout.fillWidth: true }
                    }
                    Repeater {
                        model: [{title: "Очередь", icon: "queue"}, {title: "Правила", icon: "rules"}, {title: "Категории", icon: "folder"}, {title: "История", icon: "history"}, {title: "Настройки", icon: "settings"}]
                        Button {
                            id: nav
                            required property var modelData
                            required property int index
                            Layout.fillWidth: true
                            implicitHeight: 46
                            leftPadding: 12
                            rightPadding: 12
                            checked: tabs.currentIndex === index
                            contentItem: RowLayout {
                                spacing: 12
                                LineIcon { symbol: nav.modelData.icon; ink: nav.checked ? (window.dark ? window.accent : "#aa6c09") : window.muted; Layout.preferredWidth: 21; Layout.preferredHeight: 21 }
                                Copy { text: nav.modelData.title; color: nav.checked ? window.ink : window.muted; font.bold: nav.checked; Layout.fillWidth: true }
                                Tag { visible: nav.index === 0 && queueRows.length > 0; text: queueRows.length; fill: nav.checked ? window.cardColor : window.canvasColor; implicitHeight: 23; implicitWidth: Math.max(24, children[0].implicitWidth + 12) }
                            }
                            background: Rectangle { radius: 9; color: nav.checked ? window.accentWash : (nav.hovered ? window.canvasColor : "transparent"); border.color: nav.activeFocus ? window.accent : "transparent" }
                            onClicked: tabs.currentIndex = index
                            Accessible.name: modelData.title
                        }
                    }
                    Item { Layout.fillHeight: true }
                    Rectangle { Layout.fillWidth: true; implicitHeight: 1; color: window.lineColor; Layout.bottomMargin: 8 }
                    Small { text: appState.first_run ? "ПАПКА НЕ НАСТРОЕНА" : "ПАПКА ЗАГРУЗОК"; font.pixelSize: 10; font.letterSpacing: 1; Layout.fillWidth: true }
                    Copy { text: basename(config.root); font.pixelSize: 13; elide: Text.ElideMiddle; Layout.fillWidth: true }
                    Small { text: "Локально · без облака"; font.pixelSize: 11; Layout.fillWidth: true; Layout.topMargin: 8 }
                    Button {
                        text: "О приложении"
                        Layout.fillWidth: true
                        implicitHeight: 32
                        contentItem: Small { text: parent.text; verticalAlignment: Text.AlignVCenter }
                        background: Item {}
                        onClicked: aboutDialog.open()
                    }
                }
            }
            ColumnLayout {
                Layout.fillWidth: true
                Layout.fillHeight: true
                Layout.margins: window.width < 1000 ? 22 : 30
                spacing: 22
                RowLayout {
                    Layout.fillWidth: true
                    spacing: 16
                    ColumnLayout {
                        Layout.fillWidth: true
                        Layout.minimumWidth: 0
                        spacing: 7
                        Copy { text: pageTitles[tabs.currentIndex]; font.pixelSize: 25; font.bold: true; Layout.fillWidth: true; elide: Text.ElideRight }
                        Small { text: pageHints[tabs.currentIndex]; Layout.fillWidth: true }
                    }
                    BusyIndicator { visible: busy; running: busy; Layout.preferredWidth: 25; Layout.preferredHeight: 25 }
                    Tag { text: appState.first_run ? "Настройка" : mode === "automatic" ? "Автоматически" : mode === "paused" ? "Пауза" : "Вручную"; textColor: mode === "automatic" ? window.success : window.muted; fill: window.cardColor }
                    ActionButton {
                        objectName: "modeButton"
                        text: mode === "automatic" ? "Ⅱ  Пауза" : mode === "paused" ? "Продолжить" : "Включить авто"
                        primary: mode !== "automatic"
                        enabled: !appState.first_run && (!busy || (mode === "automatic" && pendingOperation !== "set_mode"))
                        onClicked: mode === "automatic" ? setMode("paused") : requestAutomatic()
                    }
                }
                StackLayout {
                    Layout.fillWidth: true
                    Layout.fillHeight: true
                    currentIndex: tabs.currentIndex
                    ColumnLayout {
                        spacing: 16
                        RowLayout {
                            Layout.fillWidth: true
                            spacing: 10
                            ActionButton { text: "↻  Обновить"; enabled: !busy; onClicked: send({op: "scan"}) }
                            ActionButton { text: "Вернуть отложенные"; enabled: !busy; onClicked: send({op: "show_deferred"}) }
                            Item { Layout.fillWidth: true }
                            ActionButton { text: "Перенести (" + Object.keys(selected).length + ")"; primary: true; enabled: !busy && Object.keys(selected).length > 0; onClicked: confirmMove(queueRows.filter(row => selected[row.id])) }
                        }
                        Tag { visible: waitingCount > 0; text: "Автоочередь: " + waitingCount + " · ожидают готовности или следующей проверки"; fill: window.accentWash; textColor: window.ink }
                        ListView {
                            id: queueList
                            objectName: "queueList"
                            Layout.fillWidth: true
                            Layout.fillHeight: true
                            clip: true
                            spacing: 14
                            model: queueRows
                            ScrollBar.vertical: ScrollBar {}
                            delegate: Card {
                                required property var modelData
                                width: queueList.width - (queueList.contentHeight > queueList.height ? 12 : 0)
                                padding: 18
                                RowLayout {
                                    Layout.fillWidth: true
                                    spacing: 12
                                    Check { enabled: modelData.ready && !busy; checked: !!selected[modelData.id]; onToggled: select(modelData, checked); Accessible.name: "Выбрать " + modelData.name }
                                    Rectangle {
                                        Layout.preferredWidth: 42; Layout.preferredHeight: 44; radius: 9; color: window.accentWash
                                        Copy { anchors.centerIn: parent; text: String(modelData.name).split(".").pop().slice(0, 4).toUpperCase(); color: window.dark ? window.accent : "#a16b19"; font.pixelSize: 10; font.bold: true }
                                    }
                                    ColumnLayout {
                                        Layout.fillWidth: true
                                        Layout.minimumWidth: 0
                                        spacing: 5
                                        Copy { text: modelData.name; font.bold: true; Layout.fillWidth: true; elide: Text.ElideMiddle }
                                        Small { text: modelData.reason; Layout.fillWidth: true; wrapMode: Text.Wrap }
                                        Small { text: modelData.source; visible: additionalSources.length > 0; Layout.fillWidth: true; elide: Text.ElideMiddle; maximumLineCount: 1 }
                                    }
                                    Small { text: sizeLabel(modelData.size) }
                                }
                                RowLayout {
                                    Layout.fillWidth: true
                                    spacing: 12
                                    Select {
                                        Layout.preferredWidth: 200
                                        Layout.minimumWidth: 150
                                        model: config.categories
                                        textRole: "name"
                                        currentIndex: config.categories.findIndex(category => category.folder === target(modelData))
                                        displayText: currentIndex >= 0 ? config.categories[currentIndex].name : "Выберите категорию"
                                        enabled: !busy
                                        onActivated: { let value = copy(destinations); value[modelData.id] = config.categories[currentIndex].folder; destinations = value }
                                    }
                                    Small { text: target(modelData) ? destinationPath(modelData.source, target(modelData)) : "Папка назначения не выбрана"; Layout.fillWidth: true; elide: Text.ElideMiddle; maximumLineCount: 1 }
                                    ActionButton { text: "Другая папка…"; enabled: !busy; onClicked: chooseFolder("target", modelData.id, modelData.source) }
                                }
                                RowLayout {
                                    Layout.fillWidth: true
                                    spacing: 10
                                    ActionButton { text: "Создать правило"; enabled: !busy && !!target(modelData); onClicked: ruleFromDecision(modelData) }
                                    Item { Layout.fillWidth: true }
                                    ActionButton { text: "Отложить"; enabled: !busy; onClicked: send({op: "defer", id: modelData.id}) }
                                    ActionButton { text: "Переместить"; primary: true; enabled: !busy && modelData.ready && !!target(modelData); onClicked: confirmMove([modelData]) }
                                }
                            }
                            ColumnLayout {
                                anchors.centerIn: parent
                                width: Math.min(parent.width - 48, 460)
                                spacing: 14
                                visible: queueList.count === 0
                                Rectangle { Layout.alignment: Qt.AlignHCenter; width: 72; height: 72; radius: 22; color: window.accentWash; LineIcon { anchors.centerIn: parent; width: 32; height: 32; symbol: "queue"; ink: window.dark ? window.accent : "#b47b1e" } }
                                Heading { text: mode === "automatic" ? "На разборе ничего нет" : "Очередь пока пуста"; Layout.fillWidth: true; horizontalAlignment: Text.AlignHCenter }
                                Small { text: mode === "automatic" ? "Знакомые файлы разберутся по правилам. Здесь появятся только те, которым нужно ваше решение." : "Добавьте файлы в выбранную папку. Незавершённые загрузки и исключения сюда не попадут."; Layout.fillWidth: true; horizontalAlignment: Text.AlignHCenter }
                            }
                        }
                    }
                    ColumnLayout {
                        spacing: 16
                        RowLayout { Layout.fillWidth: true; Small { text: "Сверху — более точные правила. Работает первое совпадение."; Layout.fillWidth: true } ActionButton { text: "+  Новое правило"; primary: true; enabled: !busy && config.categories.length > 0; onClicked: editRule(-1, null) } }
                        ListView {
                            id: rulesList
                            Layout.fillWidth: true
                            Layout.fillHeight: true
                            spacing: 12
                            clip: true
                            model: config.rules
                            ScrollBar.vertical: ScrollBar {}
                            delegate: Card {
                                required property var modelData
                                required property int index
                                width: rulesList.width - (rulesList.contentHeight > rulesList.height ? 12 : 0)
                                padding: 18
                                RowLayout {
                                    Layout.fillWidth: true
                                    spacing: 12
                                    Check { checked: modelData.active; enabled: !busy; onToggled: { let value = copy(config); value.rules[index].active = checked; saveConfig(value) } Accessible.name: "Активировать " + modelData.name }
                                    ColumnLayout {
                                        Layout.fillWidth: true
                                        Layout.minimumWidth: 0
                                        spacing: 5
                                        Copy { text: (index + 1) + ". " + modelData.name; font.bold: true; Layout.fillWidth: true; elide: Text.ElideRight }
                                        Small { text: (modelData.condition.pattern || "Любое имя") + " · " + (modelData.condition.extensions.length ? modelData.condition.extensions.map(ext => "." + ext).join(", ") : "любое расширение"); Layout.fillWidth: true; elide: Text.ElideRight; maximumLineCount: 1 }
                                        Small { text: modelData.source || ""; visible: !!modelData.source; Layout.fillWidth: true; elide: Text.ElideMiddle; maximumLineCount: 1 }
                                    }
                                    Tag { text: modelData.action === "move" ? "Переместить" : "Предложить"; fill: modelData.action === "move" ? window.accentWash : window.sideColor; textColor: window.ink }
                                }
                                RowLayout {
                                    Layout.fillWidth: true
                                    spacing: 8
                                    Small { text: "→ " + ((config.categories.find(category => category.id === modelData.category_id) || {}).folder || "Категория не найдена"); Layout.fillWidth: true; elide: Text.ElideMiddle; maximumLineCount: 1 }
                                    ActionButton { text: "↑"; compact: true; enabled: !busy && index > 0; Accessible.name: "Поднять правило"; onClicked: { let value = copy(config), rule = value.rules.splice(index, 1)[0]; value.rules.splice(index - 1, 0, rule); saveConfig(value) } }
                                    ActionButton { text: "↓"; compact: true; enabled: !busy && index + 1 < config.rules.length; Accessible.name: "Опустить правило"; onClicked: { let value = copy(config), rule = value.rules.splice(index, 1)[0]; value.rules.splice(index + 1, 0, rule); saveConfig(value) } }
                                    ActionButton { text: "Изменить"; enabled: !busy; onClicked: editRule(index, modelData) }
                                    ActionButton { text: "Удалить"; destructive: true; enabled: !busy; onClicked: { let value = copy(config); value.rules.splice(index, 1); saveConfig(value) } }
                                }
                            }
                            Small { anchors.centerIn: parent; width: parent.width - 48; horizontalAlignment: Text.AlignHCenter; visible: rulesList.count === 0; text: "Добавьте первое правило. До этого файлы останутся на месте." }
                        }
                    }
                    ColumnLayout {
                        spacing: 16
                        RowLayout { Layout.fillWidth: true; Small { text: "Каждая категория — название и папка назначения."; Layout.fillWidth: true } ActionButton { text: "+  Новая категория"; primary: true; enabled: !busy; onClicked: { categoryDialog.categoryIndex = -1; categoryDialog.categoryId = "category-" + Date.now(); categoryName.text = ""; categoryFolder.text = ""; categoryDialog.open() } } }
                        ListView {
                            id: categoriesList
                            Layout.fillWidth: true
                            Layout.fillHeight: true
                            spacing: 12
                            clip: true
                            model: config.categories
                            ScrollBar.vertical: ScrollBar {}
                            delegate: Card {
                                required property var modelData
                                required property int index
                                width: categoriesList.width - (categoriesList.contentHeight > categoriesList.height ? 12 : 0)
                                padding: 18
                                RowLayout {
                                    Layout.fillWidth: true
                                    spacing: 14
                                    Rectangle { Layout.preferredWidth: 44; Layout.preferredHeight: 44; radius: 11; color: window.accentWash; LineIcon { anchors.centerIn: parent; symbol: "folder"; ink: window.dark ? window.accent : "#aa7215" } }
                                    ColumnLayout { Layout.fillWidth: true; Layout.minimumWidth: 0; spacing: 5; Heading { text: modelData.name; Layout.fillWidth: true; elide: Text.ElideRight } Small { text: modelData.folder; Layout.fillWidth: true; elide: Text.ElideMiddle; maximumLineCount: 1 } }
                                    ActionButton { text: "Изменить"; enabled: !busy; onClicked: { categoryDialog.categoryIndex = index; categoryDialog.categoryId = modelData.id; categoryName.text = modelData.name; categoryFolder.text = modelData.folder; categoryDialog.open() } }
                                    ActionButton { text: "Удалить"; destructive: true; enabled: !busy && !config.rules.some(rule => rule.category_id === modelData.id); onClicked: { let value = copy(config); value.categories.splice(index, 1); saveConfig(value) } }
                                }
                            }
                        }
                        Small { text: "Используемую категорию можно удалить после изменения связанных с ней правил."; Layout.fillWidth: true }
                    }
                    ColumnLayout {
                        spacing: 16
                        Small { text: "Последние 500 операций. Отмена доступна, если файл не изменён, а исходное место свободно."; Layout.fillWidth: true }
                        ListView {
                            id: historyList
                            Layout.fillWidth: true
                            Layout.fillHeight: true
                            spacing: 12
                            clip: true
                            model: appState.history || []
                            ScrollBar.vertical: ScrollBar {}
                            delegate: Card {
                                required property var modelData
                                width: historyList.width - (historyList.contentHeight > historyList.height ? 12 : 0)
                                padding: 18
                                RowLayout {
                                    Layout.fillWidth: true
                                    spacing: 14
                                    ColumnLayout {
                                        Layout.fillWidth: true
                                        Layout.minimumWidth: 0
                                        spacing: 7
                                        RowLayout { Layout.fillWidth: true; Copy { text: basename(modelData.source); font.bold: true; Layout.fillWidth: true; elide: Text.ElideMiddle } Tag { text: stateName(modelData.state); textColor: modelData.state === "moved" ? window.success : modelData.state === "needs_review" || modelData.state === "error" ? window.danger : window.muted } }
                                        Small { text: modelData.source; Layout.fillWidth: true; elide: Text.ElideMiddle; maximumLineCount: 1 }
                                        Small { text: "→ " + modelData.destination; Layout.fillWidth: true; elide: Text.ElideMiddle; maximumLineCount: 1 }
                                        Small { text: modelData.created + " UTC"; Layout.fillWidth: true }
                                        Small { text: modelData.message; visible: !!modelData.message; color: modelData.state === "error" || modelData.state === "needs_review" ? window.danger : window.muted; Layout.fillWidth: true }
                                    }
                                    ActionButton { text: "Отменить"; enabled: !busy && modelData.state === "moved"; onClicked: send({op: "undo", id: modelData.id}) }
                                }
                            }
                            ColumnLayout { anchors.centerIn: parent; width: Math.min(parent.width - 48, 420); spacing: 12; visible: historyList.count === 0; LineIcon { Layout.alignment: Qt.AlignHCenter; width: 38; height: 38; symbol: "history" } Heading { text: "История начнётся с первого переноса"; horizontalAlignment: Text.AlignHCenter; Layout.fillWidth: true; wrapMode: Text.Wrap } Small { text: "Здесь будут исходный и конечный путь, результат и кнопка отмены."; horizontalAlignment: Text.AlignHCenter; Layout.fillWidth: true } }
                        }
                    }
                    ScrollView {
                        id: settingsScroll
                        Layout.fillWidth: true
                        Layout.fillHeight: true
                        clip: true
                        contentWidth: availableWidth
                        ColumnLayout {
                            width: settingsScroll.availableWidth
                            spacing: 16
                            Card {
                                Layout.fillWidth: true
                                Heading { text: "Папка загрузок" }
                                Small { text: appState.first_run ? "Выберите папку. Сначала покажем её содержимое в предпросмотре — файлы останутся на месте." : "Проверяется верхний уровень папки. Подпапки назначения не просматриваются."; Layout.fillWidth: true }
                                RowLayout {
                                    Layout.fillWidth: true
                                    spacing: 10
                                    Input { id: rootField; objectName: "rootField"; enabled: !busy; Layout.fillWidth: true; Layout.minimumWidth: 160; placeholderText: "Выберите папку загрузок"; onTextEdited: rootDirty = true; Accessible.name: "Папка загрузок" }
                                    ActionButton { id: chooseRootButton; objectName: "chooseRootButton"; text: "Выбрать…"; enabled: !busy; onClicked: chooseFolder("root", "") }
                                    ActionButton { id: saveRootButton; objectName: "saveRootButton"; text: "Сохранить"; primary: true; enabled: !busy && rootField.text.length > 0; onClicked: { let value = copy(config); value.root = rootField.text; saveConfig(value) } }
                                }
                                RowLayout {
                                    Layout.fillWidth: true
                                    Check { text: "Авторазбор"; enabled: !busy && !appState.first_run; checked: config.root_automatic; onToggled: updateSource(-1, "root_automatic", checked) }
                                    Check { text: "Только новые файлы"; enabled: !busy && !appState.first_run; checked: !!config.only_new; onToggled: updateSource(-1, "only_new", checked) }
                                }
                                Small { text: "Назначение — подпапки на том же диске. Новые категории и папки можно добавить во вкладке «Категории»."; Layout.fillWidth: true }
                            }
                            Card {
                                Layout.fillWidth: true
                                visible: !appState.first_run
                                RowLayout {
                                    Layout.fillWidth: true
                                    Heading { text: "Дополнительные папки"; Layout.fillWidth: true }
                                    ActionButton { text: "+  Добавить папку"; enabled: !busy; onClicked: chooseFolder("source", "") }
                                }
                                Small { text: "Для каждой папки можно включить авторазбор. Общая пауза действует на все папки. «Только новые» оставляет файлы, уже лежащие в папке, на ручной разбор."; Layout.fillWidth: true }
                                Repeater {
                                    model: additionalSources
                                    ColumnLayout {
                                        required property var modelData
                                        required property int index
                                        Layout.fillWidth: true
                                        spacing: 5
                                        Rectangle { implicitHeight: 1; Layout.fillWidth: true; color: window.lineColor }
                                        Copy { text: modelData.path; font.bold: true; Layout.fillWidth: true; elide: Text.ElideMiddle }
                                        RowLayout {
                                            Layout.fillWidth: true
                                            Check { text: "Авторазбор"; enabled: !busy; checked: modelData.automatic; onToggled: updateSource(index, "automatic", checked) }
                                            Check { text: "Только новые"; enabled: !busy; checked: modelData.only_new; onToggled: updateSource(index, "only_new", checked) }
                                            Item { Layout.fillWidth: true }
                                            ActionButton {
                                                text: "Убрать"
                                                enabled: !busy
                                                onClicked: {
                                                    let value = copy(config)
                                                    value.sources.splice(index, 1)
                                                    saveConfig(value)
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                            Card {
                                Layout.fillWidth: true
                                Heading { text: "Как разбирать файлы" }
                                RowLayout {
                                    Layout.fillWidth: true
                                    spacing: 10
                                    ActionButton { text: "Вручную"; primary: mode === "manual"; enabled: !busy && !appState.first_run; onClicked: setMode("manual") }
                                    ActionButton { text: "Автоматически"; primary: mode === "automatic"; enabled: !busy && !appState.first_run; onClicked: requestAutomatic() }
                                    ActionButton { text: "Пауза"; primary: mode === "paused"; enabled: !appState.first_run && mode !== "manual" && pendingOperation !== "set_mode"; onClicked: setMode("paused") }
                                }
                                Small { text: "В автоматическом режиме знакомые файлы с действием «Переместить» разбираются сами. Остальные остаются в очереди. Проверка выполняется каждые 15 секунд."; Layout.fillWidth: true }
                                Small { text: "Перед переносом ждём минуту без изменений. Для загрузчиков, которые могут надолго приостанавливаться, выбирайте папку завершённых загрузок."; Layout.fillWidth: true }
                                Check { text: "Запускать при входе в систему"; enabled: !busy && !appState.first_run; checked: !!config.autostart; onToggled: send({op: "set_autostart", enabled: checked}) }
                                Small { text: tray.available ? "После закрытия окна приложение продолжит работу в трее." : "В этой среде трей недоступен: для фоновой работы оставьте окно открытым."; Layout.fillWidth: true }
                            }
                            Card {
                                Layout.fillWidth: true
                                RowLayout {
                                    Layout.fillWidth: true
                                    spacing: 18
                                    ColumnLayout { Layout.fillWidth: true; spacing: 6; Heading { text: "Оформление" } Small { text: "Цвета обновляются сразу, без перезапуска."; Layout.fillWidth: true } }
                                    Select { objectName: "themeSelect"; Layout.preferredWidth: 190; enabled: !busy; model: ["Системное", "Светлое", "Тёмное"]; currentIndex: ["system", "light", "dark"].indexOf(themeChoice); onActivated: setTheme(["system", "light", "dark"][currentIndex]) }
                                }
                            }
                            Card {
                                Layout.fillWidth: true
                                RowLayout { Layout.fillWidth: true; ColumnLayout { Layout.fillWidth: true; spacing: 6; Heading { text: "Исключения" } Small { text: "Проверяются перед правилами. Совпавшие файлы остаются на месте."; Layout.fillWidth: true } } ActionButton { text: "+  Добавить"; enabled: !busy; onClicked: { exclusionExt.text = ""; exclusionPattern.text = ""; exclusionDialog.open() } } }
                                Repeater {
                                    model: config.exclusions
                                    RowLayout { required property var modelData; required property int index; Layout.fillWidth: true; Small { text: (modelData.pattern || "Любое имя") + " · " + (modelData.extensions.join(", ") || "любые расширения"); Layout.fillWidth: true; elide: Text.ElideRight; maximumLineCount: 1 } ActionButton { text: "Удалить"; destructive: true; enabled: !busy; onClicked: { let value = copy(config); value.exclusions.splice(index, 1); saveConfig(value) } } }
                                }
                                Small { text: config.exclusions.length === 0 ? "Дополнительных исключений пока нет." : "Временные файлы загрузок (.part, .crdownload и другие) пропускаются всегда."; Layout.fillWidth: true }
                            }
                            Card {
                                Layout.fillWidth: true
                                Heading { text: "Перенос настроек" }
                                Small { text: "Сохраните категории и правила в JSON или загрузите готовые. Импорт не перемещает файлы и включает ручной режим."; Layout.fillWidth: true }
                                RowLayout { Layout.fillWidth: true; spacing: 10; ActionButton { text: "Импортировать JSON…"; enabled: !busy; onClicked: importFile.open() } ActionButton { text: "Экспортировать JSON…"; enabled: !busy; onClicked: exportFile.open() } Item { Layout.fillWidth: true } }
                            }
                            Item { implicitHeight: 2; Layout.fillWidth: true }
                        }
                    }
                }
                Rectangle {
                    Layout.fillWidth: true
                    implicitHeight: statusRow.implicitHeight + 22
                    radius: 9
                    color: hasError ? (window.dark ? "#3f2520" : "#fff0ea") : window.cardColor
                    border.color: hasError ? window.danger : window.lineColor
                    RowLayout {
                        id: statusRow
                        anchors.left: parent.left
                        anchors.right: parent.right
                        anchors.verticalCenter: parent.verticalCenter
                        anchors.margins: 14
                        spacing: 10
                        Rectangle { width: 6; height: 6; radius: 3; color: hasError ? window.danger : window.success }
                        Small { text: notice; color: hasError ? window.danger : window.muted; Layout.fillWidth: true; maximumLineCount: 3; elide: Text.ElideRight }
                    }
                }
            }
        }
    }

    function checkRootGeometry() {
        let input = rootField.mapToItem(page, 0, 0)
        let choose = chooseRootButton.mapToItem(page, 0, 0)
        let save = saveRootButton.mapToItem(page, 0, 0)
        return input.x >= 0 && input.x + rootField.width + 8 <= choose.x && choose.x + chooseRootButton.width + 8 <= save.x && save.x + saveRootButton.width <= page.width - 18 && Math.abs(choose.y - save.y) < 1 && Math.abs(chooseRootButton.height - saveRootButton.height) < 1
    }
    Timer {
        property int step: 0
        property int previousReplies: 0
        interval: 450
        repeat: true
        running: Qt.application.arguments.includes("--smoke-test") && !Qt.application.arguments.includes("--automation-test")
        onTriggered: {
            if (busy) return
            let first = Qt.application.arguments.includes("--first-run-test")
            if (step === 0) {
                previousReplies = receivedReplies
                backend.request(JSON.stringify({op: "init"}))
                backend.request(JSON.stringify({op: "init"}))
            } else if (step === 1) {
                if (receivedReplies < previousReplies + 2) return
                console.log("BRIDGE_REPEAT_READY")
                tabs.currentIndex = 4
            } else if (step === 2) {
                if (!checkRootGeometry() || !chooseRootButton.enabled || !saveRootButton.enabled) { console.error("UI_SMOKE_FAILED root_controls"); Qt.quit(); return }
                console.log("UI_LAYOUT_READY width=" + window.width)
                if (!first) { editRule(0, config.rules[0]); ruleCategory.popup.open() }
            } else if (step === 3) {
                if (!first && (ruleDialog.y < 0 || ruleDialog.y + ruleDialog.height > window.height)) { console.error("UI_SMOKE_FAILED rule_dialog_bounds"); Qt.quit(); return }
                ruleCategory.popup.close(); ruleDialog.close()
                chooseFolder("root", "")
            } else if (step === 4) {
                folders.close(); importFile.open()
            } else if (step === 5) {
                importFile.close()
                console.log("UI_DIALOGS_READY")
                if (tray.available) {
                    window.close()
                    if (window.visible) { console.error("UI_SMOKE_FAILED close_to_tray"); Qt.quit(); return }
                    showQueue()
                    console.log("UI_TRAY_READY")
                }
                let args = Qt.application.arguments, theme = args.indexOf("--test-theme")
                if (theme >= 0) themeChoice = args[theme + 1]
                tabs.currentIndex = first ? 4 : 0
            } else if (step === 6) {
                if (busy || appState.error || (!first && (queueList.count !== 2 || rulesList.count !== 1 || categoriesList.count !== 1))) { console.error("UI_SMOKE_FAILED " + JSON.stringify(appState)); Qt.quit(); return }
                let args = Qt.application.arguments, screenshot = args.indexOf("--screenshot")
                if (screenshot >= 0) page.grabToImage(function(result) { if (!result.saveToFile(args[screenshot + 1])) console.error("SCREENSHOT_FAILED"); console.log("QML_READY queue=" + queueList.count); Qt.quit() })
                else { console.log("QML_READY queue=" + queueList.count); Qt.quit() }
                stop()
            }
            step += 1
        }
    }
    Timer {
        property double started: 0
        property double lastTest: 0
        property bool baselineRequested: false
        property bool stopping: false
        interval: 400
        repeat: true
        running: Qt.application.arguments.includes("--automation-test")
        onTriggered: {
            if (busy) return
            if (hasError) { console.error("UI_SMOKE_FAILED automation " + notice); Qt.quit(); return }
            if (started === 0) {
                if (appState.first_run) return
                if (!baselineRequested) {
                    baselineRequested = true
                    updateSource(-1, "only_new", true)
                    return
                }
                if (!config.only_new || additionalSources.length !== 2 || !queueRows.some(row => row.name === "existing.png" && row.manual_only)) {
                    console.error("UI_SMOKE_FAILED baseline"); Qt.quit(); return
                }
                tabs.currentIndex = 1
                started = Date.now()
                requestAutomatic()
                if (automaticCandidates.length !== 1 || automaticCandidates[0].name !== "Scoped.png" || !autoConfirm.visible) {
                    console.error("UI_SMOKE_FAILED auto_preview_sources"); Qt.quit(); return
                }
                autoConfirm.close()
                setMode("automatic")
                console.log("BASELINE_READY")
                return
            }
            if (stopping) {
                if (mode !== "paused") { console.error("UI_SMOKE_FAILED pause_after_automation"); Qt.quit(); return }
                console.log("AUTOMATION_READY elapsed=" + Math.round((Date.now() - started) / 1000))
                Qt.quit()
                stop()
                return
            }
            let moved = (appState.history || []).filter(entry => entry.state === "moved").map(entry => basename(entry.source))
            if (moved.includes("Screenshot.png") && moved.includes("Scoped.png")) {
                if (!queueRows.some(row => row.name === "Manual.png") || !queueRows.some(row => row.name === "existing.png")) {
                    console.error("UI_SMOKE_FAILED manual_queue_sources"); Qt.quit(); return
                }
                if (Date.now() - started < 59000 || tabs.currentIndex !== 1) { console.error("UI_SMOKE_FAILED automation_timing"); Qt.quit(); return }
                stopping = true
                setMode("paused")
                return
            }
            if (Date.now() - started > 90000) { console.error("UI_SMOKE_FAILED automation_timeout"); Qt.quit(); stop(); return }
            if (Date.now() - lastTest > 5000) {
                lastTest = Date.now()
                send({op: "test", condition: {extensions: ["png"], pattern: ""}})
            }
        }
    }

}
