#pragma once

#include <QCoreApplication>
#include <QProcess>
#include <QString>

inline QProcess &window_process() {
    static auto *process = new QProcess(QCoreApplication::instance());
    return *process;
}

inline bool window_running() {
    return window_process().state() != QProcess::NotRunning;
}

inline void show_window_process(bool tray) {
    auto &process = window_process();
    if (window_running()) {
        process.write("{\"window_action\":\"show\"}\n");
        return;
    }
    auto args = QCoreApplication::arguments().mid(1);
    args.append(QStringLiteral("--queue-window"));
    if (tray) args.append(QStringLiteral("--has-tray"));
    process.setProcessChannelMode(QProcess::ForwardedErrorChannel);
    process.start(QCoreApplication::applicationFilePath(), args);
}

inline QString next_window_command() {
    auto &process = window_process();
    return process.canReadLine() ? QString::fromUtf8(process.readLine()).trimmed() : QString();
}

inline void send_window_message(const QString &message) {
    if (window_running()) window_process().write(message.toUtf8() + '\n');
}
