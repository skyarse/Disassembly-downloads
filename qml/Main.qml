import QtQuick
import QtQuick.Controls
import QtQuick.Layouts
import QtQuick.Dialogs
import net.skyarse.DownloadsTriage

ApplicationWindow {
    id: window
    width: 1120
    height: 760
    minimumWidth: 850
    minimumHeight: 570
    visible: true
    title: "Загрузки на разбор"
    property var appState: ({queue: [], history: [], matches: []})
    property var config: ({root: "", categories: [], rules: [], exclusions: []})
    property bool busy: false
    property var selected: ({})
    property var destinations: ({})
    property string folderPurpose: ""
    property string folderToken: ""
    property var moveItems: []
    property var pendingDecision: null
    property string notice: "Загрузка настроек…"

    function send(command) {
        if (busy) return
        busy = true
        backend.request(JSON.stringify(command))
    }
    function copy(value) { return JSON.parse(JSON.stringify(value)) }
    function saveConfig(value) { send({op: "save", config: value}) }
    function localPath(url) { return decodeURIComponent(url.toString().replace(/^file:\/\//, "")) }
    function folderUrl(path) { return "file://" + path.split("/").map(encodeURIComponent).join("/") }
    function target(row) { return destinations[row.id] !== undefined ? destinations[row.id] : row.destination }
    function select(row, checked) {
        let value = copy(selected)
        if (checked) value[row.id] = true
        else delete value[row.id]
        selected = value
    }
    function chooseFolder(purpose, token) {
        folderPurpose = purpose
        folderToken = token || ""
        folders.currentFolder = folderUrl(config.root || "/")
        folders.open()
    }
    function confirmMove(rows) {
        moveItems = rows.filter(row => row.ready).map(row => ({id: row.id, folder: target(row), name: row.name}))
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
        ruleDialog.ruleId = row ? row.id : "rule-" + Date.now()
        ruleDialog.open()
    }
    function conditionFromEditor() {
        return {extensions: extensions.text.split(/[,;\s]+/).map(x => x.replace(/^\./, "").toLowerCase()).filter(x => x.length), pattern: pattern.text}
    }
    function ruleFromDecision(row) {
        let folder = target(row)
        let relative = folder.indexOf(config.root + "/") === 0 ? folder.slice(config.root.length + 1) : folder
        let category = config.categories.find(c => c.folder === folder || c.folder === relative)
        if (!category) {
            pendingDecision = {name: row.name, destination: folder}
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
    }
    function stateName(value) {
        return ({moved: "Перемещено", undone: "Отменено", planned: "Начато", undo_planned: "Отмена начата", needs_review: "Нужна проверка", error: "Не завершено"})[value] || value
    }

    Backend {
        id: backend
        onResponseChanged: {
            let response
            try { response = JSON.parse(responseValue()) } catch (error) { busy = false; notice = "Ошибка ответа: " + error; return }
            window.busy = false
            let wasFirstRun = !!window.appState.first_run
            if (response.config) {
                window.appState = response
                window.config = response.config
                rootField.text = response.config.root
                window.selected = ({})
                window.destinations = ({})
            }
            if (response.error) notice = response.error
            else if (response.message) notice = response.message
            else notice = response.first_run ? "Выберите папку и сохраните настройки. Файлы перемещаются только после подтверждения." : "Ручной режим. Файлы ожидают минуту без изменений."
            if (ruleDialog.visible && ruleDialog.awaitingTest) {
                ruleDialog.awaitingTest = false
                ruleDialog.tested = !response.error
                ruleDialog.matchesText = response.error || (response.matches.length ? response.matches.join("\n") : "Совпадений нет.")
            }
            if (response.first_run) tabs.currentIndex = 4
            else if (wasFirstRun) tabs.currentIndex = 0
            if (window.pendingDecision && !categoryDialog.visible && !response.error) {
                let decision = window.pendingDecision
                window.pendingDecision = null
                ruleFromDecision(decision)
            }
        }
        function responseValue() { return backend.response }
    }
    Component.onCompleted: send({op: "init"})
    Timer { interval: 100; running: true; repeat: true; onTriggered: backend.poll() }
    Timer {
        interval: 15000
        running: !appState.first_run
        repeat: true
        onTriggered: if (!busy && tabs.currentIndex === 0 && !Object.keys(selected).length && !Object.keys(destinations).length && !ruleDialog.visible && !moveConfirm.visible && !folders.visible) send({op: "scan"})
    }
    Timer {
        interval: 2500
        running: Qt.application.arguments.indexOf("--smoke-test") >= 0
        onTriggered: {
            if (busy || appState.error || queueList.count !== 2 || rulesList.count !== 1 || categoriesList.count !== 1) {
                console.error("UI_SMOKE_FAILED " + JSON.stringify(appState))
                Qt.quit()
                return
            }
            let screenshot = Qt.application.arguments.indexOf("--screenshot")
            if (screenshot >= 0) {
                page.grabToImage(function(result) {
                    if (!result.saveToFile(Qt.application.arguments[screenshot + 1])) console.error("SCREENSHOT_FAILED")
                    console.log("QML_READY queue=" + (window.appState.queue ? window.appState.queue.length : 0))
                    Qt.quit()
                })
            } else {
                console.log("QML_READY queue=" + (appState.queue ? appState.queue.length : 0))
                Qt.quit()
            }
        }
    }
    FolderDialog {
        id: folders
        title: folderPurpose === "root" ? "Папка загрузок" : "Подпапка назначения внутри загрузок"
        onAccepted: {
            let path = localPath(selectedFolder)
            if (folderPurpose === "root") rootField.text = path
            else if (folderPurpose === "category") categoryFolder.text = path
            else {
                let value = copy(destinations)
                value[folderToken] = path
                destinations = value
            }
        }
    }
    FileDialog {
        id: importFile
        title: "Импорт настроек"
        nameFilters: ["Настройки JSON (*.json)"]
        onAccepted: importConfirm.open()
    }
    FileDialog {
        id: exportFile
        title: "Экспорт настроек под новым именем"
        fileMode: FileDialog.SaveFile
        nameFilters: ["Настройки JSON (*.json)"]
        defaultSuffix: "json"
        onAccepted: send({op: "export", path: localPath(selectedFile)})
    }
    Dialog {
        id: importConfirm
        anchors.centerIn: parent
        title: "Заменить настройки?"
        modal: true
        standardButtons: Dialog.Ok | Dialog.Cancel
        Label { text: "Категории, правила и папка будут заменены после проверки JSON.\nСами файлы останутся на месте."; wrapMode: Text.Wrap }
        onAccepted: send({op: "import", path: localPath(importFile.selectedFile)})
    }
    Dialog {
        id: moveConfirm
        anchors.centerIn: parent
        width: Math.min(window.width - 80, 700)
        title: "Подтвердить перенос"
        modal: true
        standardButtons: Dialog.Ok | Dialog.Cancel
        ScrollView {
            width: parent.width
            height: Math.min(300, confirmationText.implicitHeight + 20)
            clip: true
            TextArea {
                id: confirmationText
                readOnly: true
                wrapMode: TextEdit.Wrap
                text: moveItems.map(row => row.name + " → " + row.folder).join("\n")
            }
        }
        onAccepted: send({op: "move", items: moveItems})
    }
    Dialog {
        id: categoryDialog
        property int categoryIndex: -1
        property string categoryId: ""
        title: categoryIndex < 0 ? "Новая категория" : "Изменить категорию"
        anchors.centerIn: parent
        modal: true
        width: 600
        standardButtons: Dialog.Save | Dialog.Cancel
        onRejected: window.pendingDecision = null
        ColumnLayout {
            width: parent.width
            Label { text: "Название" }
            TextField { id: categoryName; Layout.fillWidth: true }
            Label { text: "Подпапка: относительный путь или полный путь внутри загрузок" }
            RowLayout {
                TextField { id: categoryFolder; Layout.fillWidth: true; placeholderText: "Аниме/Название" }
                Button { text: "Выбрать…"; onClicked: chooseFolder("category", "") }
            }
        }
        onAccepted: {
            let value = copy(config)
            let item = {id: categoryId, name: categoryName.text, folder: categoryFolder.text}
            if (categoryIndex < 0) value.categories.push(item)
            else value.categories[categoryIndex] = item
            saveConfig(value)
        }
    }
    Dialog {
        id: ruleDialog
        property int ruleIndex: -1
        property string ruleId: ""
        property bool tested: false
        property bool awaitingTest: false
        property string matchesText: ""
        title: ruleIndex < 0 ? "Создать правило" : "Изменить правило"
        anchors.centerIn: parent
        width: 670
        modal: true
        closePolicy: Popup.CloseOnEscape
        ColumnLayout {
            width: parent.width
            Label { text: "Название" }
            TextField { id: ruleName; Layout.fillWidth: true; onTextEdited: ruleDialog.tested = false }
            Label { text: "Расширения через запятую; пусто — любые" }
            TextField { id: extensions; Layout.fillWidth: true; placeholderText: "mkv, mp4"; onTextEdited: ruleDialog.tested = false }
            Label { text: "Шаблон имени (* — любой текст, ? — один символ); регистр не важен" }
            TextField { id: pattern; Layout.fillWidth: true; placeholderText: "Screenshot*.png"; onTextEdited: ruleDialog.tested = false }
            Label { text: "Если заполнены оба поля, должны совпасть оба условия."; wrapMode: Text.Wrap }
            RowLayout {
                ComboBox { id: ruleCategory; Layout.fillWidth: true; model: config.categories; textRole: "name"; onActivated: ruleDialog.tested = false }
                ComboBox { id: ruleAction; model: ["Переместить", "Предложить"] }
                CheckBox { id: ruleActive; text: "Активно" }
            }
            Label { text: "Первое совпадение побеждает. Новое правило добавится первым.\nВ этой версии оба действия требуют ручного подтверждения."; wrapMode: Text.Wrap }
            Button {
                text: "Проверить совпадения"
                enabled: !busy
                onClicked: {
                    ruleDialog.awaitingTest = true
                    send({op: "test", condition: conditionFromEditor()})
                }
            }
            ScrollView {
                Layout.fillWidth: true
                Layout.preferredHeight: 150
                TextArea { readOnly: true; wrapMode: TextEdit.Wrap; text: ruleDialog.matchesText; placeholderText: "Перед сохранением проверьте условие." }
            }
            RowLayout {
                Layout.alignment: Qt.AlignRight
                Button { text: "Отмена"; onClicked: ruleDialog.close() }
                Button {
                    text: "Сохранить"
                    enabled: ruleDialog.tested && !busy && config.categories.length > 0
                    onClicked: {
                        let value = copy(config)
                        let rule = {id: ruleDialog.ruleId, name: ruleName.text, condition: conditionFromEditor(), category_id: config.categories[ruleCategory.currentIndex].id, action: ruleAction.currentIndex === 0 ? "move" : "suggest", active: ruleActive.checked}
                        if (ruleDialog.ruleIndex < 0) value.rules.unshift(rule)
                        else value.rules[ruleDialog.ruleIndex] = rule
                        ruleDialog.close()
                        saveConfig(value)
                    }
                }
            }
        }
    }
    Dialog {
        id: exclusionDialog
        anchors.centerIn: parent
        title: "Добавить исключение"
        width: 550
        modal: true
        standardButtons: Dialog.Save | Dialog.Cancel
        ColumnLayout {
            width: parent.width
            Label { text: "Файлы, совпавшие с исключением, не попадают в очередь."; wrapMode: Text.Wrap }
            TextField { id: exclusionExt; Layout.fillWidth: true; placeholderText: "Расширения: bak, log" }
            TextField { id: exclusionPattern; Layout.fillWidth: true; placeholderText: "Шаблон: keep-*" }
        }
        onAccepted: {
            let value = copy(config)
            value.exclusions.push({extensions: exclusionExt.text.split(/[,;\s]+/).map(x => x.replace(/^\./, "").toLowerCase()).filter(x => x.length), pattern: exclusionPattern.text})
            saveConfig(value)
        }
    }

    ColumnLayout {
        id: page
        anchors.fill: parent
        anchors.margins: 16
        spacing: 12
        RowLayout {
            Label { text: "Загрузки на разбор"; font.pixelSize: 26; font.bold: true; Layout.fillWidth: true }
            Label { text: "Ручной режим"; opacity: 0.7 }
            BusyIndicator { running: busy; visible: busy; Layout.preferredWidth: 30; Layout.preferredHeight: 30 }
        }
        Label { text: config.root; elide: Text.ElideMiddle; Layout.fillWidth: true; opacity: 0.75 }
        TabBar {
            id: tabs
            Layout.fillWidth: true
            TabButton { text: "Очередь (" + (appState.queue ? appState.queue.length : 0) + ")" }
            TabButton { text: "Правила" }
            TabButton { text: "Категории" }
            TabButton { text: "История" }
            TabButton { text: "Настройки" }
        }
        StackLayout {
            currentIndex: tabs.currentIndex
            Layout.fillWidth: true
            Layout.fillHeight: true
            ColumnLayout {
                RowLayout {
                    Button { text: "Обновить"; enabled: !busy; onClicked: send({op: "scan"}) }
                    Button { text: "Вернуть отложенные"; enabled: !busy; onClicked: send({op: "show_deferred"}) }
                    Item { Layout.fillWidth: true }
                    Button {
                        text: "Подтвердить выбранные"
                        enabled: !busy && Object.keys(selected).length > 0
                        onClicked: confirmMove(appState.queue.filter(row => selected[row.id]))
                    }
                }
                ListView {
                    id: queueList
                    Layout.fillWidth: true
                    Layout.fillHeight: true
                    spacing: 8
                    clip: true
                    model: appState.queue || []
                    ScrollBar.vertical: ScrollBar {}
                    delegate: Frame {
                        required property var modelData
                        width: queueList.width - 12
                        ColumnLayout {
                            width: parent.width
                            RowLayout {
                                CheckBox { enabled: modelData.ready && !busy; checked: !!selected[modelData.id]; onToggled: select(modelData, checked) }
                                Label { text: modelData.name; font.bold: true; elide: Text.ElideMiddle; Layout.fillWidth: true }
                                Label { text: (modelData.size / 1048576).toFixed(2) + " МБ"; opacity: 0.7 }
                            }
                            Label { text: modelData.reason; wrapMode: Text.Wrap; Layout.fillWidth: true }
                            RowLayout {
                                ComboBox {
                                    Layout.preferredWidth: 190
                                    model: config.categories
                                    textRole: "name"
                                    currentIndex: config.categories.findIndex(c => c.folder === target(modelData))
                                    displayText: currentIndex >= 0 ? config.categories[currentIndex].name : "Выберите категорию"
                                    enabled: !busy
                                    onActivated: { let value = copy(destinations); value[modelData.id] = config.categories[currentIndex].folder; destinations = value }
                                }
                                Label { text: target(modelData) || "Папка не выбрана"; elide: Text.ElideMiddle; Layout.fillWidth: true }
                                Button { text: "Папка…"; enabled: !busy; onClicked: chooseFolder("target", modelData.id) }
                                Button { text: "Создать правило…"; enabled: !busy && !!target(modelData); onClicked: ruleFromDecision(modelData) }
                                Button { text: "Отложить"; enabled: !busy; onClicked: send({op: "defer", id: modelData.id}) }
                                Button { text: "Перенести"; enabled: !busy && modelData.ready && !!target(modelData); onClicked: confirmMove([modelData]) }
                            }
                        }
                    }
                    Label { anchors.centerIn: parent; visible: queueList.count === 0; text: "Очередь пуста. Подпапки и временные загрузки пропускаются."; wrapMode: Text.Wrap }
                }
            }
            ColumnLayout {
                Button { text: "Добавить правило"; enabled: !busy && config.categories.length > 0; onClicked: editRule(-1, null) }
                ListView {
                    id: rulesList
                    Layout.fillWidth: true
                    Layout.fillHeight: true
                    spacing: 6
                    clip: true
                    model: config.rules
                    ScrollBar.vertical: ScrollBar {}
                    delegate: Frame {
                        required property var modelData
                        required property int index
                        width: rulesList.width - 12
                        RowLayout {
                            width: parent.width
                            CheckBox {
                                checked: modelData.active
                                enabled: !busy
                                onToggled: { let value = copy(config); value.rules[index].active = checked; saveConfig(value) }
                            }
                            ColumnLayout {
                                Layout.fillWidth: true
                                Label { text: (index + 1) + ". " + modelData.name; font.bold: true }
                                Label { text: (modelData.condition.pattern || "*") + "  ·  " + (modelData.condition.extensions.join(", ") || "любые расширения"); elide: Text.ElideMiddle; Layout.fillWidth: true }
                            }
                            Button { text: "↑"; enabled: !busy && index > 0; onClicked: { let value = copy(config); let rule = value.rules.splice(index, 1)[0]; value.rules.splice(index - 1, 0, rule); saveConfig(value) } }
                            Button { text: "↓"; enabled: !busy && index + 1 < config.rules.length; onClicked: { let value = copy(config); let rule = value.rules.splice(index, 1)[0]; value.rules.splice(index + 1, 0, rule); saveConfig(value) } }
                            Button { text: "Изменить"; enabled: !busy; onClicked: editRule(index, modelData) }
                            Button { text: "Удалить"; enabled: !busy; onClicked: { let value = copy(config); value.rules.splice(index, 1); saveConfig(value) } }
                        }
                    }
                }
            }
            ColumnLayout {
                Button {
                    text: "Добавить категорию"
                    enabled: !busy
                    onClicked: { categoryDialog.categoryIndex = -1; categoryDialog.categoryId = "category-" + Date.now(); categoryName.text = ""; categoryFolder.text = ""; categoryDialog.open() }
                }
                ListView {
                    id: categoriesList
                    Layout.fillWidth: true
                    Layout.fillHeight: true
                    clip: true
                    spacing: 6
                    model: config.categories
                    delegate: Frame {
                        required property var modelData
                        required property int index
                        width: categoriesList.width - 12
                        RowLayout {
                            width: parent.width
                            Label { text: modelData.name + " → " + modelData.folder; Layout.fillWidth: true; elide: Text.ElideMiddle }
                            Button { text: "Изменить"; enabled: !busy; onClicked: { categoryDialog.categoryIndex = index; categoryDialog.categoryId = modelData.id; categoryName.text = modelData.name; categoryFolder.text = modelData.folder; categoryDialog.open() } }
                            Button { text: "Удалить"; enabled: !busy && !config.rules.some(r => r.category_id === modelData.id); onClicked: { let value = copy(config); value.categories.splice(index, 1); saveConfig(value) } }
                        }
                    }
                    ScrollBar.vertical: ScrollBar {}
                }
                Label { text: "Для удаления используемой категории сначала измените или удалите связанные правила."; wrapMode: Text.Wrap; Layout.fillWidth: true }
            }
            ColumnLayout {
                Label { text: "Последние 500 операций. Время указано в UTC."; opacity: 0.7 }
                ListView {
                    id: historyList
                    Layout.fillWidth: true
                    Layout.fillHeight: true
                    clip: true
                    spacing: 6
                    model: appState.history || []
                    ScrollBar.vertical: ScrollBar {}
                    delegate: Frame {
                        required property var modelData
                        width: historyList.width - 12
                        RowLayout {
                            width: parent.width
                            ColumnLayout {
                                Layout.fillWidth: true
                                Label { text: modelData.created + " · " + stateName(modelData.state); font.bold: true }
                                Label { text: modelData.source; elide: Text.ElideMiddle; Layout.fillWidth: true }
                                Label { text: "→ " + modelData.destination; elide: Text.ElideMiddle; Layout.fillWidth: true }
                                Label { text: modelData.message; visible: !!modelData.message; wrapMode: Text.Wrap; Layout.fillWidth: true }
                            }
                            Button { text: "Отменить"; enabled: !busy && modelData.state === "moved"; onClicked: send({op: "undo", id: modelData.id}) }
                        }
                    }
                }
            }
            ScrollView {
                clip: true
                ColumnLayout {
                    width: parent.width
                    spacing: 14
                    Label { text: "Папка загрузок"; font.bold: true }
                    RowLayout {
                        TextField { id: rootField; Layout.fillWidth: true; placeholderText: "/home/user/Downloads" }
                        Button { text: "Выбрать…"; enabled: !busy; onClicked: chooseFolder("root", "") }
                        Button { text: "Сохранить"; enabled: !busy; onClicked: { let value = copy(config); value.root = rootField.text; saveConfig(value) } }
                    }
                    Label { text: "Обходится только верхний уровень. Назначение — подпапка на том же диске.\nПроверка каждые 15 секунд во вкладке очереди. Автоматическое перемещение ещё не включено."; wrapMode: Text.Wrap; Layout.fillWidth: true }
                    RowLayout {
                        Button { text: "Импорт JSON…"; enabled: !busy; onClicked: importFile.open() }
                        Button { text: "Экспорт JSON…"; enabled: !busy; onClicked: exportFile.open() }
                    }
                    Label { text: "Исключения проверяются перед правилами"; font.bold: true }
                    Button { text: "Добавить исключение"; enabled: !busy; onClicked: { exclusionExt.text = ""; exclusionPattern.text = ""; exclusionDialog.open() } }
                    Repeater {
                        model: config.exclusions
                        RowLayout {
                            required property var modelData
                            required property int index
                            Label { text: (modelData.pattern || "*") + " · " + (modelData.extensions.join(", ") || "любые расширения"); Layout.fillWidth: true }
                            Button { text: "Удалить"; enabled: !busy; onClicked: { let value = copy(config); value.exclusions.splice(index, 1); saveConfig(value) } }
                        }
                    }
                    Label { text: "О приложении"; font.bold: true }
                    Label {
                        text: "Загрузки на разбор · 0.1.0, предварительная версия\nGPL-3.0-only. Приложение предоставляется без гарантий.\nИсходники: https://github.com/skyarse/Disassembly-downloads\nRust + CXX-Qt + Qt 6; настройки JSON, журнал SQLite. Сетевых запросов нет."
                        wrapMode: Text.Wrap
                        Layout.fillWidth: true
                    }
                }
            }
        }
        Frame {
            Layout.fillWidth: true
            Label { width: parent.width; text: notice; wrapMode: Text.Wrap; color: appState.error ? "#d65f5f" : palette.text }
        }
    }
}
