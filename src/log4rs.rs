use log4rs::{
    append::console::ConsoleAppender,
    config::{Appender, Config, Root},
    encode::pattern::PatternEncoder,
};

pub fn init() {
    // 创建一个控制台 Appender
    let stdout = ConsoleAppender::builder()
        .encoder(Box::new(PatternEncoder::new(
            "{d(%Y-%m-%d %H:%M:%S)} [{t}] {h({l})} - {m}{n}",
        )))
        .build();

    // 构建 log4rs 配置，仅输出到控制台
    let config = Config::builder()
        .appender(Appender::builder().build("stdout", Box::new(stdout)))
        .build(
            Root::builder()
                .appender("stdout")
                .build(log::LevelFilter::Info), // 设置日志级别
        )
        .expect("构建 log4rs 配置失败");

    // 初始化日志器
    log4rs::init_config(config).expect("初始化 log4rs 失败");
}
