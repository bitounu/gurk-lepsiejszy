//! Signal Messenger client for terminal

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use std::fs;

use anyhow::{Context, anyhow};
use chrono::{DateTime, Utc};
use clap::Parser;
use crossterm::{
    event::{
        DisableFocusChange, DisableMouseCapture, EnableFocusChange, EnableMouseCapture,
        Event as CEvent, EventStream, KeyEvent, KeyEventKind, MouseEvent,
    },
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use gurk::signal::LocalPool;
use gurk::{app::App, config::Config};
use gurk::{backoff::Backoff, passphrase::Passphrase};
use gurk::{
    onboarding,
    storage::{SqliteStorage, Storage, sync_from_signal},
};
use gurk::{signal, ui};
use presage::libsignal_service::content::Content;
use ratatui::{
    Terminal,
    backend::{Backend, ClearType, CrosstermBackend},
};
use tokio::{runtime, select};
use tokio_stream::StreamExt;
use tracing::{debug, error, info};
use url::Url;

const TARGET_FPS: u64 = 144;
const RECEIPT_TICK_PERIOD: u64 = 144;
const FRAME_BUDGET: Duration = Duration::from_millis(1000 / TARGET_FPS);
const SAVE_BUDGET: Duration = Duration::from_millis(1000);
const RECEIPT_BUDGET: Duration = Duration::from_millis(RECEIPT_TICK_PERIOD * 1000 / TARGET_FPS);

#[derive(Debug, Parser)]
#[command(author, version, about, long_about = None)]
#[command(group(clap::ArgGroup::new("backup_mode").args(["restore_cloud_backup", "backup_report"])))]
struct Args {
    /// Restore existing Signal Secure Backup into local history, then exit (requires a linked account).
    #[arg(long, conflicts_with = "relink")]
    restore_cloud_backup: bool,
    /// Show privacy-preserving diagnostics from the last import, without contacting Signal.
    #[arg(long, conflicts_with_all = ["relink", "restore_cloud_backup", "backup_dry_run", "backup_messages_only", "backup_key_file"])]
    backup_report: bool,
    /// Authenticate and inventory the archive without changing local chat history.
    #[arg(long, requires = "restore_cloud_backup")]
    backup_dry_run: bool,
    /// Import message text without downloading media; encrypted manifests remain for later resume.
    #[arg(long, requires = "restore_cloud_backup")]
    backup_messages_only: bool,
    /// Private managed cache/staging directory for resumable backup import.
    #[arg(long, requires = "backup_mode")]
    backup_work_dir: Option<PathBuf>,
    /// Private recovery-key file (otherwise prompted without echo); never pass the key as an argument.
    #[arg(long, requires = "restore_cloud_backup")]
    backup_key_file: Option<PathBuf>,
    /// Enables logging to `gurk.log` in the current working directory according to RUST_LOG
    #[arg(short, long = "verbose")]
    verbosity: bool,
    /// Relinks the device (helpful when device was unlinked)
    #[arg(long)]
    relink: bool,
    /// Passphrase to use for encrypting the database
    ///
    /// When omitted, passphrase is read from the config file, passphrase_command, and if missing, prompted for.
    #[arg(long, short, conflicts_with = "passphrase_command")]
    passphrase: Option<Passphrase>,
    /// Get a passphrase from external command. For example `pass`(password-store)
    ///
    /// When omitted, passphrase_command is read from the env "GURK_PASSPHRASE_COMMAND".
    #[arg(long, conflicts_with = "passphrase")]
    passphrase_command: Option<String>,
    /// Path to config file (overrides default config search paths)
    ///
    /// Can also be set via GURK_CONFIG environment variable.
    #[arg(long, short, env = "GURK_CONFIG")]
    config: Option<PathBuf>,
    /// Path to data directory (overrides config file's data_dir)
    ///
    /// Can also be set via GURK_DATA_DIR environment variable.
    #[arg(long, env = "GURK_DATA_DIR")]
    data_dir: Option<PathBuf>,
}

fn main() -> anyhow::Result<()> {
    let mut args = Args::parse();

    let _guard = if args.verbosity {
        let file_appender = tracing_appender::rolling::never("./", "gurk.log");
        let (non_blocking, guard) = tracing_appender::non_blocking(file_appender);
        tracing_subscriber::fmt::fmt()
            .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
            .with_writer(non_blocking)
            .with_ansi(false)
            .init();
        Some(guard)
    } else {
        None
    };

    log_panics::init();

    let (mut config, passphrase) = match args
        .config
        .take()
        .map(|p| {
            Config::load(&p).with_context(|| format!("failed to load config from {}", p.display()))
        })
        .transpose()?
        .or(Config::load_installed().context("failed to load config")?)
    {
        Some(config) => {
            let mut config = config.report_deprecated_keys();
            let passphrase = Passphrase::get(
                args.passphrase.take(),
                args.passphrase_command.take(),
                &mut config,
            )?;
            (config, passphrase)
        }
        None if args.restore_cloud_backup || args.backup_report => {
            anyhow::bail!("Configure and link gurk before importing a backup")
        }
        None => onboarding::run()?,
    };

    // CLI --data-dir overrides config file and clears any hardcoded paths
    if let Some(data_dir) = args.data_dir.take() {
        config.data_dir = data_dir;
        // Clear sqlite config so db path is derived from data_dir
        config.sqlite = None;
    }

    // Live clients share this lock; restoration requires exclusive database access.
    gurk::backup::staging::private_directory(&config.data_dir)?;
    let mut lock_options = fs::OpenOptions::new();
    lock_options
        .read(true)
        .write(true)
        .create(true)
        .truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        lock_options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    let _instance_lock = lock_options.open(config.data_dir.join("gurk-instance.lock"))?;
    if args.restore_cloud_backup {
        _instance_lock
            .try_lock()
            .context("close other Gurk instances before restoring a backup")?;
    } else {
        _instance_lock
            .try_lock_shared()
            .context("a backup import is running; wait before opening Gurk")?;
    }

    let runtime = runtime::Builder::new_multi_thread()
        .thread_stack_size(8 * 1024 * 1024)
        .worker_threads(2)
        .enable_all()
        .build()?;
    if args.backup_report {
        runtime.block_on(gurk::backup::cli::report(
            config,
            passphrase,
            args.backup_work_dir,
        ))
    } else if args.restore_cloud_backup {
        runtime.block_on(gurk::backup::cli::run(
            config,
            passphrase,
            gurk::backup::cli::RestoreOptions {
                work_dir: args.backup_work_dir,
                key_file: args.backup_key_file,
                dry_run: args.backup_dry_run,
                messages_only: args.backup_messages_only,
            },
        ))
    } else {
        runtime.block_on(run(config, passphrase, args.relink))
    }
}

async fn is_online() -> bool {
    tokio::net::TcpStream::connect("detectportal.firefox.com:80")
        .await
        .is_ok()
}

#[allow(clippy::large_enum_variant)]
#[derive(Debug)]
pub enum Event {
    Redraw,
    Focus(bool),
    Click(MouseEvent),
    Input(KeyEvent),
    Paste(String),
    Message(Box<Content>),
    Resize { cols: u16, rows: u16 },
    Quit(Option<anyhow::Error>),
    ContactSynced(DateTime<Utc>),
    Tick,
    AppEvent(gurk::event::Event),
}

async fn run(config: Config, passphrase: Passphrase, relink: bool) -> anyhow::Result<()> {
    fs::create_dir_all(&config.data_dir).context("could not create data dir")?;
    let local_pool = LocalPool::new();

    let mut signal_manager =
        signal::ensure_linked_device(relink, local_pool.clone(), &config, &passphrase).await?;

    let mut storage: Box<dyn Storage> = {
        let url = match config
            .sqlite
            .as_ref()
            .map(|sqlite_config| sqlite_config.url.clone())
        {
            Some(url) => url,
            None => Url::from_file_path(config.gurk_db_path())
                .map_err(|_| anyhow!("failed to convert gurk db path to url"))?,
        };

        debug!(%url, "opening sqlite data storage");
        let sqlite_storage = SqliteStorage::maybe_encrypt_and_open(&url, &passphrase, false)
            .await
            .with_context(|| format!("failed to open sqlite data storage at: {url}"))?;
        Box::new(sqlite_storage)
    };

    sync_from_signal(&*signal_manager, &mut *storage).await;

    let (mut app, mut app_events) = App::try_new(config, signal_manager.clone_boxed(), storage)?;
    app.populate_names_cache().await;

    let (tx, mut rx) = tokio::sync::mpsc::channel::<Event>(100);
    let inner_tx = tx.clone();
    local_pool.spawn(move || async move {
        let mut backoff = Backoff::new();
        loop {
            let mut messages = if !is_online().await {
                tokio::time::sleep(std::time::Duration::from_secs(10)).await;
                continue;
            } else {
                match signal_manager.receive_messages().await {
                    Ok(messages) => {
                        info!("connected and listening for incoming messages");
                        messages
                    }
                    Err(e) => {
                        let e = e.context(
                            "failed to initialize the stream of Signal messages.\n\
                            Maybe the device was unlinked? Please try to restart with \
                            '--relink` flag.",
                        );
                        inner_tx
                            .send(Event::Quit(Some(e)))
                            .await
                            .expect("logic error: events channel closed");
                        return;
                    }
                }
            };

            while let Some(message) = messages.next().await {
                backoff.reset();
                inner_tx
                    .send(Event::Message(message))
                    .await
                    .expect("logic error: events channel closed")
            }

            let after = backoff.get();
            error!(?after, "messages channel disconnected. trying to reconnect");
            tokio::time::sleep(after).await;
        }
    });

    enable_raw_mode()?;
    let _raw_mode_guard = scopeguard::guard((), |_| {
        disable_raw_mode().unwrap();
    });

    let mut stdout = std::io::stdout();
    execute!(
        stdout,
        EnterAlternateScreen,
        EnableMouseCapture,
        EnableFocusChange
    )?;

    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    // Clear before spawning the event reader: Terminal::clear queries the cursor position (ESC[6n)
    // and reads the reply back from stdin. Spawned event reader consumes that reply and the query
    // times out.
    terminal.clear()?;
    app.initialize_image_viewer();

    // Stored reader so it can be aborted before launching an external editor (otherwise both
    // processes compete for stdin and the editor ends up missing keystrokes).
    let spawn_event_reader = {
        let tx = tx.clone();
        move || {
            let tx = tx.clone();
            tokio::spawn(async move {
                let mut reader = EventStream::new().fuse();
                while let Some(event) = reader.next().await {
                    match event {
                        Ok(CEvent::FocusGained) => {
                            let _ = tx.send(Event::Focus(true)).await;
                        }
                        Ok(CEvent::FocusLost) => {
                            let _ = tx.send(Event::Focus(false)).await;
                        }
                        Ok(CEvent::Key(key)) => tx.send(Event::Input(key)).await.unwrap(),
                        Ok(CEvent::Resize(cols, rows)) => {
                            tx.send(Event::Resize { cols, rows }).await.unwrap()
                        }
                        Ok(CEvent::Mouse(button)) => tx.send(Event::Click(button)).await.unwrap(),
                        Ok(CEvent::Paste(content)) => tx.send(Event::Paste(content)).await.unwrap(),
                        _ => (),
                    }
                }
            })
        }
    };
    let mut event_reader_handle = spawn_event_reader();

    let mut res = Ok(()); // result on quit
    let mut last_render_at = Instant::now();
    let mut last_save_at = Instant::now();
    let is_render_spawned = Arc::new(AtomicBool::new(false));

    let tick_tx = tx.clone();
    // Tick to trigger receipt sending
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(RECEIPT_BUDGET);
        loop {
            interval.tick().await;
            if tick_tx.send(Event::Tick).await.is_err() {
                break;
            }
        }
    });

    let mut expire_tick_counter: u64 = 0;

    loop {
        // render
        let left_frame_budget = FRAME_BUDGET.checked_sub(last_render_at.elapsed());
        if let Some(budget) = left_frame_budget {
            // skip frames that render too fast
            if !is_render_spawned.load(Ordering::Relaxed) {
                let tx = tx.clone();
                let is_render_spawned = is_render_spawned.clone();
                is_render_spawned.store(true, Ordering::Relaxed);
                tokio::spawn(async move {
                    // Redraw message is needed to make sure that we render the skipped frame
                    // if it was the last frame in the rendering budget window.
                    tokio::time::sleep(budget).await;
                    tx.send(Event::Redraw)
                        .await
                        .expect("logic error: events channel closed");
                    is_render_spawned.store(false, Ordering::Relaxed);
                });
            }
        } else {
            if app.should_clear {
                // Don't use Terminal::clear: it queries the cursor position, and the reply is
                // consumed by the running event reader, so the query times out.
                terminal.backend_mut().clear_region(ClearType::All)?;
                terminal.swap_buffers();
                app.should_clear = false;
            }
            terminal.draw(|f| ui::draw(f, &mut app))?;
            last_render_at = Instant::now();
        }

        let event = select! {
            v = rx.recv() => v,
            v = app_events.recv() => v.map(Event::AppEvent),
        };

        match event {
            Some(Event::Focus(focused)) => {
                app.terminal_focused = focused;
            }
            Some(Event::Tick) => {
                app.step_private_reads();
                app.step_receipts();
                app.expire_typing_indicators();
                expire_tick_counter += 1;
                if expire_tick_counter >= 5 {
                    expire_tick_counter = 0;
                    app.activate_expire_timers();
                    app.expire_messages();
                }
            }
            Some(Event::Click(event)) => app.on_mouse(event, terminal.get_frame().area()),
            Some(Event::Input(
                event @ KeyEvent {
                    kind: KeyEventKind::Press,
                    ..
                },
            )) => app.on_key(event).await?,
            Some(Event::Input(..)) => {}
            Some(Event::Paste(content)) => {
                app.on_paste(&content);
            }
            Some(Event::Message(content)) => {
                if let Err(e) = app.on_message(content).await {
                    error!("failed on incoming message: {}", e);
                }
            }
            Some(Event::Resize { .. }) | Some(Event::Redraw) => {
                // will just redraw the app
            }
            Some(Event::Quit(e)) => {
                if let Some(e) = e {
                    res = Err(e);
                };
                break;
            }
            Some(Event::ContactSynced(at)) => {
                let mut metadata = app.storage.metadata();
                metadata.contacts_sync_request_at.replace(at);
                app.storage.store_metadata(&metadata);
                info!(%at, "synced contacts");
            }
            Some(Event::AppEvent(event)) => {
                if let Err(error) = app.handle_event(event) {
                    error!(%error, "failed to handle app event");
                }
            }
            None => {
                break;
            }
        }

        app.ensure_message_window_filled();

        if app.open_editor_requested || app.editor_preview_requested.is_some() {
            app.open_editor_requested = false;
            let preview_path = app.editor_preview_requested.take();
            let is_file_preview = preview_path.is_some();
            let initial_content = app.input.data.clone();

            // Abort event reader BEFORE releasing the terminal so editor gets
            // exclusive access to stdin (no stolen keystrokes).
            event_reader_handle.abort();
            let _ = event_reader_handle.await;

            execute!(
                terminal.backend_mut(),
                LeaveAlternateScreen,
                DisableMouseCapture,
                DisableFocusChange
            )?;
            terminal.show_cursor()?;
            disable_raw_mode()?;

            // editor on blocking thread (edit::edit spawns subprocess + waits)
            let edit_result =
                tokio::task::spawn_blocking(move || -> anyhow::Result<Option<String>> {
                    if let Some(path) = preview_path {
                        gurk::editor_preview::preview_file(path)?;
                        Ok(None)
                    } else {
                        Ok(Some(edit::edit(initial_content)?))
                    }
                })
                .await
                .unwrap_or_else(|error| Err(error.into()));

            enable_raw_mode()?;
            execute!(
                terminal.backend_mut(),
                EnterAlternateScreen,
                EnableMouseCapture,
                EnableFocusChange
            )?;
            terminal.backend_mut().clear_region(ClearType::All)?;
            terminal.swap_buffers();
            event_reader_handle = spawn_event_reader();
            if is_file_preview {
                app.finish_editor_preview(
                    edit_result
                        .map(|_| ())
                        .map_err(|error| format!("{error:#}")),
                );
            } else {
                match edit_result {
                    Ok(Some(text)) => {
                        app.input
                            .replace_text(text.trim_end_matches('\n').to_string());
                    }
                    Err(error) => app.report_editor_error(format!("{error:#}")),
                    Ok(None) => {}
                }
            }
        }

        if last_save_at.elapsed() > SAVE_BUDGET || app.should_quit {
            if let Err(error) = app.persist_composer() {
                app.report_composer_save_error(error);
                app.should_quit = false;
            }
            app.storage.save();
            last_save_at = Instant::now();
        }

        if app.should_quit {
            break;
        }
    }

    if let Err(error) = app.persist_composer() {
        res = Err(error.context("failed to preserve message drafts on exit"));
    }

    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture,
        DisableFocusChange
    )
    .unwrap();
    terminal.show_cursor().unwrap();

    res
}

#[cfg(test)]
mod backup_args_tests {
    use super::*;
    #[test]
    fn offline_report_has_its_own_mode_and_cannot_restore_or_relink() {
        let args = Args::try_parse_from([
            "gurk",
            "--backup-report",
            "--backup-work-dir",
            "/tmp/report",
        ])
        .unwrap();
        assert!(args.backup_report);
        assert!(!args.restore_cloud_backup);
        for arguments in [
            vec!["gurk", "--backup-report", "--restore-cloud-backup"],
            vec!["gurk", "--backup-report", "--relink"],
            vec!["gurk", "--backup-report", "--backup-dry-run"],
            vec!["gurk", "--backup-report", "--backup-key-file", "/tmp/key"],
            vec!["gurk", "--backup-work-dir", "/tmp/report"],
        ] {
            assert!(
                Args::try_parse_from(&arguments).is_err(),
                "unexpectedly accepted {arguments:?}"
            );
        }
        assert!(
            Args::try_parse_from([
                "gurk",
                "--restore-cloud-backup",
                "--backup-work-dir",
                "/tmp/cache"
            ])
            .is_ok()
        );
    }
}
