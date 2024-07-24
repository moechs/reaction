use log::{Level, Metadata};

pub struct SimpleLogger {
    loglevel: Level,
}

impl log::Log for SimpleLogger {
    fn enabled(&self, metadata: &Metadata) -> bool {
        metadata.level() <= self.loglevel
    }

    fn log(&self, record: &log::Record) {
        if self.enabled(record.metadata()) {
            eprintln!(
                "{} {}",
                match record.level() {
                    Level::Error => "ERROR",
                    Level::Warn => "WARN ",
                    Level::Info => "INFO ",
                    Level::Debug => "DEBUG",
                    Level::Trace => "TRACE",
                },
                record.args().to_string().trim()
            );
        }
    }

    fn flush(&self) {}
}

impl SimpleLogger {
    pub fn init(loglevel: log::Level) -> Result<(), log::SetLoggerError> {
        log::set_boxed_logger(Box::new(SimpleLogger { loglevel }))
            .map(|()| log::set_max_level(loglevel.to_level_filter()))
    }
}
