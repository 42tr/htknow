use std::{env, str::FromStr};

use log4rs::{
    append::console::ConsoleAppender, config::{Appender, Config, Logger, Root}, encode::pattern::PatternEncoder
};

fn parse_rust_log(rust_log: &str) -> (Option<log::LevelFilter>, Vec<(String, log::LevelFilter)>) {
    let mut default_level = None;
    let mut loggers = Vec::new();

    for directive in rust_log.split(',') {
        let directive = directive.trim();
        if directive.is_empty() {
            continue;
        }

        let (target, level_str) = match directive.split_once('=') {
            Some((target, level)) => (Some(target.trim()), level.trim()),
            None => (None, directive),
        };

        let Ok(level) = log::LevelFilter::from_str(level_str) else {
            continue;
        };

        match target {
            Some(target) if !target.is_empty() => {
                loggers.push((target.to_string(), level));
            }
            _ => {
                default_level = Some(level);
            }
        }
    }

    (default_level, loggers)
}

pub fn init() {
    // 创建一个控制台 Appender
    let stdout = ConsoleAppender::builder()
        .encoder(Box::new(PatternEncoder::new("{d(%Y-%m-%d %H:%M:%S)} [{t}] {h({l})} - {m}{n}")))
        .build();

    let mut root_level = log::LevelFilter::Info;
    let mut logger_levels = Vec::new();
    if let Ok(rust_log) = env::var("RUST_LOG") {
        let (default_level, parsed_loggers) = parse_rust_log(&rust_log);
        logger_levels = parsed_loggers;
        if let Some(level) = default_level {
            root_level = level;
        } else if !logger_levels.is_empty() {
            root_level = log::LevelFilter::Off;
        }
    }

    // Lance 的逐次 I/O span 在 INFO 级别会产生大量日志。显式配置相同 target 时仍允许覆盖。
    for (target, level) in [("lance", log::LevelFilter::Warn), ("tracing::span", log::LevelFilter::Warn)] {
        if !logger_levels.iter().any(|(configured_target, _)| configured_target == target) {
            logger_levels.push((target.to_string(), level));
        }
    }

    // 构建 log4rs 配置，仅输出到控制台
    let mut config_builder = Config::builder().appender(Appender::builder().build("stdout", Box::new(stdout)));
    for (target, level) in logger_levels {
        config_builder = config_builder.logger(Logger::builder().build(target, level));
    }
    let config =
        config_builder.build(Root::builder().appender("stdout").build(root_level)).expect("构建 log4rs 配置失败");

    // 初始化日志器
    log4rs::init_config(config).expect("初始化 log4rs 失败");
}
