#pragma once
#include "rust/cxx.h"
#include <QApplication>
#include <QGuiApplication>
#include <QIcon>
#include <memory>
#include <vector>

inline std::unique_ptr<QGuiApplication> create_application(rust::Vec<rust::String> arguments) {
    static std::vector<QByteArray> storage;
    static std::vector<char*> argv;
    static int argc;
    for (const auto& argument : arguments) {
        storage.emplace_back(argument.data(), static_cast<qsizetype>(argument.size()));
    }
    for (auto& argument : storage) argv.push_back(argument.data());
    argv.push_back(nullptr);
    argc = static_cast<int>(storage.size());
    auto app = std::make_unique<QApplication>(argc, argv.data());
    QGuiApplication::setDesktopFileName(QStringLiteral("net.skyarse.DownloadsTriage"));
    QGuiApplication::setApplicationDisplayName(QStringLiteral("Загрузки на разбор"));
    app->setWindowIcon(QIcon(QStringLiteral(":/icons/app.svg")));
    return app;
}
