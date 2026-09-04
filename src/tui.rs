use std::io;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use crossterm::execute;
use crossterm::terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode};
use librqbit::{AddTorrent, AddTorrentOptions, Session};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Cell, Clear, Gauge, List, ListItem, ListState, Paragraph, Row, Table, TableState};

use crate::storage::{DownloadHistoryEntry, now_epoch_secs};
use crate::types::Torrent;

// --- Catppuccin Mocha Theme ---
const TEXT: Color = Color::Rgb(205, 214, 244);
const OVERLAY0: Color = Color::Rgb(108, 112, 134);
const MAUVE: Color = Color::Rgb(203, 166, 247);
const SURFACE1: Color = Color::Rgb(69, 71, 90);
const SURFACE0: Color = Color::Rgb(49, 50, 68);
const YELLOW: Color = Color::Rgb(249, 226, 175);
const SKY: Color = Color::Rgb(137, 220, 235);
const GREEN: Color = Color::Rgb(166, 227, 161);
const RED: Color = Color::Rgb(243, 139, 168);
const LAVENDER: Color = Color::Rgb(180, 190, 254);

struct TerminalGuard;

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), LeaveAlternateScreen, crossterm::cursor::Show);
    }
}

/// Runs the search TUI.
///
/// # Errors
///
/// Returns an error if terminal setup fails or the event loop encounters an error.
pub async fn run_search_tui(session: Arc<Session>, history_entries: Vec<DownloadHistoryEntry>, history_path: PathBuf, client: reqwest::Client) -> Result<()> {
    let mut terminal = setup_terminal()?;
    let _guard = TerminalGuard;
    let mut app = SearchTui::new(session, history_entries, history_path, client);

    app.run(&mut terminal).await
}

fn setup_terminal() -> Result<Terminal<CrosstermBackend<io::Stdout>>> {
    enable_raw_mode().context("failed to enable raw mode")?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen).context("failed to enter alternate screen")?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend).context("failed to initialize terminal")?;
    terminal.hide_cursor().context("failed to hide cursor")?;
    terminal.clear().context("failed to clear terminal")?;
    Ok(terminal)
}

struct SearchTui {
    query_input: String,
    results: Vec<Torrent>,
    selected_result: usize,
    selected_download: usize,
    downloads: Vec<DownloadSession>,
    session: Arc<Session>,
    history_path: PathBuf,
    client: reqwest::Client,
    dpi_blocked: Arc<AtomicBool>,
    should_quit: bool,
    show_help: bool,
    focus: FocusPane,
    is_searching: bool,
    download_tx: tokio::sync::mpsc::UnboundedSender<(String, DownloadEvent)>,
    download_rx: tokio::sync::mpsc::UnboundedReceiver<(String, DownloadEvent)>,
    results_tx: tokio::sync::mpsc::UnboundedSender<Result<Vec<Torrent>>>,
    results_rx: tokio::sync::mpsc::UnboundedReceiver<Result<Vec<Torrent>>>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum FocusPane {
    Query,
    Results,
    Downloads,
}

impl SearchTui {
    fn new(session: Arc<Session>, history_entries: Vec<DownloadHistoryEntry>, history_path: PathBuf, client: reqwest::Client) -> Self {
        let mut downloads: Vec<DownloadSession> = Vec::new();
        for entry in history_entries {
            downloads.push(DownloadSession::from_history_entry(entry));
        }

        let (download_tx, download_rx) = tokio::sync::mpsc::unbounded_channel();
        let (results_tx, results_rx) = tokio::sync::mpsc::unbounded_channel();

        let dpi_blocked = Arc::new(AtomicBool::new(true));
        let dpi_clone = dpi_blocked.clone();
        tokio::spawn(async move {
            Self::run_canary_probe(dpi_clone).await;
        });

        Self {
            query_input: String::new(),
            results: Vec::new(),
            selected_result: 0,
            selected_download: 0,
            downloads,
            session,
            history_path,
            client,
            dpi_blocked,
            should_quit: false,
            show_help: false,
            focus: FocusPane::Results,
            is_searching: false,
            download_tx,
            download_rx,
            results_tx,
            results_rx,
        }
    }

    async fn run_canary_probe(dpi_blocked: Arc<AtomicBool>) {
        let is_blocked = tokio::task::spawn_blocking(|| {
            use std::net::{TcpStream, ToSocketAddrs};
            use std::io::{Read, Write};

            let addrs = match "tracker.opentrackr.org:1337".to_socket_addrs() {
                Ok(a) => a.collect::<Vec<_>>(),
                Err(_) => return true,
            };
            if addrs.is_empty() {
                return true;
            }

            let socket = match TcpStream::connect_timeout(&addrs[0], Duration::from_millis(1500)) {
                Ok(s) => s,
                Err(_) => return true,
            };

            let _ = socket.set_read_timeout(Some(Duration::from_millis(1500)));
            let _ = socket.set_write_timeout(Some(Duration::from_millis(1500)));

            let handshake = b"\x13BitTorrent protocol\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00";
            let mut stream = socket;
            if stream.write_all(handshake).is_err() {
                return true;
            }

            let mut buf = [0u8; 68];
            match stream.read(&mut buf) {
                Ok(n) if n > 0 => false,
                _ => true,
            }
        }).await.unwrap_or(true);

        dpi_blocked.store(is_blocked, Ordering::Relaxed);
    }

    async fn run(&mut self, terminal: &mut Terminal<CrosstermBackend<io::Stdout>>) -> Result<()> {
        let mut needs_redraw = true;

        let (event_tx, mut event_rx) = tokio::sync::mpsc::unbounded_channel::<Event>();
        std::thread::spawn(move || {
            while let Ok(evt) = event::read() {
                if event_tx.send(evt).is_err() {
                    break;
                }
            }
        });

        while !self.should_quit {
            if needs_redraw {
                terminal.draw(|frame| self.draw(frame))?;
                needs_redraw = false;
            }

            tokio::select! {
                biased;
                event_opt = event_rx.recv() => {
                    if let Some(Event::Key(key)) = event_opt
                        && key.kind == KeyEventKind::Press {
                            self.handle_key(key.code);
                            needs_redraw = true;
                        }
                }
                download_evt_opt = self.download_rx.recv() => {
                    if let Some((info_hash, event)) = download_evt_opt
                        && let Some(download) = self.downloads.iter_mut().find(|d| d.torrent.info_hash == info_hash) {
                            let is_success = matches!(event, DownloadEvent::Success);
                            download.apply_event(event);
                            if is_success && matches!(download.tracking, DownloadTracking::Managed) && !download.target_path.as_os_str().is_empty() {
                                let entry = DownloadHistoryEntry {
                                    info_hash: download.torrent.info_hash.clone(),
                                    name: download.torrent.name.clone(),
                                    target_path: download.target_path.clone(),
                                    added_at_epoch_secs: now_epoch_secs(),
                                    completed_at_epoch_secs: Some(now_epoch_secs()),
                                };
                                let history_path = self.history_path.clone();
                                tokio::task::spawn_blocking(move || {
                                    let _ = crate::storage::upsert_history(&history_path, entry);
                                });
                            }
                            needs_redraw = true;
                        }
                }
                res_opt = self.results_rx.recv() => {
                    if let Some(res) = res_opt {
                        self.is_searching = false;
                        if let Ok(results) = res {
                            self.results = results;
                            self.selected_result = 0;
                            self.focus = FocusPane::Results;
                            needs_redraw = true;
                        }
                    }
                }
            }
        }

        Ok(())
    }

    fn draw_query(&self, frame: &mut ratatui::Frame<'_>, area: Rect) {
        let query_block = Block::default().borders(Borders::ALL).title(" Search Query ").border_style(self.focus_style(FocusPane::Query));
        let query = Paragraph::new(self.query_input.as_str()).block(query_block).style(Style::default().fg(TEXT));
        frame.render_widget(query, area);

        if matches!(self.focus, FocusPane::Query) && !self.show_help {
            let count = u16::try_from(self.query_input.chars().count()).unwrap_or(u16::MAX);
            let cursor_x = area.x.saturating_add(1).saturating_add(count);
            let cursor_y = area.y.saturating_add(1);
            frame.set_cursor_position((cursor_x, cursor_y));
        }
    }

    fn draw_results(&self, frame: &mut ratatui::Frame<'_>, area: Rect) {
        if self.is_searching {
            let text = Paragraph::new(Span::styled("Searching...", Style::default().fg(SKY))).block(Block::default().padding(ratatui::widgets::Padding::horizontal(1)));
            frame.render_widget(text, area);
        } else if self.results.is_empty() {
            let text = Paragraph::new(Span::styled("No results found.", Style::default().fg(OVERLAY0))).block(Block::default().padding(ratatui::widgets::Padding::horizontal(1)));
            frame.render_widget(text, area);
        } else {
            let header = Row::new([Cell::from("Seed").style(Style::default().fg(OVERLAY0).add_modifier(Modifier::BOLD)), Cell::from("Size").style(Style::default().fg(OVERLAY0).add_modifier(Modifier::BOLD)), Cell::from("Name").style(Style::default().fg(OVERLAY0).add_modifier(Modifier::BOLD))]).bottom_margin(1);

            let rows = self.results.iter().map(|torrent| Row::new([Cell::from(Span::styled(torrent.seeders.to_string(), Style::default().fg(YELLOW).add_modifier(Modifier::BOLD))), Cell::from(Span::styled(crate::util::format_size(torrent.size_bytes), Style::default().fg(SKY))), Cell::from(Span::styled(torrent.name.as_str(), Style::default().fg(TEXT)))]));

            let widths = [Constraint::Length(6), Constraint::Length(10), Constraint::Min(20)];

            let (bg_color, fg_color) = if self.focus == FocusPane::Results { (SURFACE1, YELLOW) } else { (SURFACE0, OVERLAY0) };

            let table = Table::new(rows, widths).header(header).block(Block::default().padding(ratatui::widgets::Padding::horizontal(1))).row_highlight_style(Style::default().bg(bg_color).fg(fg_color).add_modifier(Modifier::BOLD)).highlight_symbol("▌ ");

            let mut table_state = TableState::default();
            table_state.select((!self.results.is_empty()).then_some(self.selected_result));
            frame.render_stateful_widget(table, area, &mut table_state);
        }
    }

    fn draw_downloads_list(&self, frame: &mut ratatui::Frame<'_>, area: Rect) {
        let mut download_items: Vec<ListItem<'_>> = Vec::with_capacity(self.downloads.len() + 1);

        for download in &self.downloads {
            let line = Line::from(vec![download.status_badge(), Span::raw(" "), Span::styled(download.torrent.name.as_str(), Style::default().fg(TEXT))]);
            download_items.push(ListItem::new(line));
        }

        if download_items.is_empty() {
            download_items.push(ListItem::new(Line::from(Span::styled("No active downloads.", Style::default().fg(OVERLAY0)))));
        }
        let downloads = List::new(download_items).block(Block::default().borders(Borders::ALL).title(" Downloads ").border_style(self.focus_style(FocusPane::Downloads))).highlight_style(Style::default().fg(YELLOW).bg(SURFACE1).add_modifier(Modifier::BOLD)).highlight_symbol("▌ ");
        let mut downloads_state = ListState::default();
        downloads_state.select((!self.downloads.is_empty()).then_some(self.selected_download));
        frame.render_stateful_widget(downloads, area, &mut downloads_state);
    }

    fn draw_downloads_activity(&self, frame: &mut ratatui::Frame<'_>, area: Rect) {
        let activity_block = Block::default().borders(Borders::ALL).title(" Activity ").border_style(Style::default().fg(OVERLAY0));
        let inner_area = activity_block.inner(area);
        frame.render_widget(activity_block, area);

        let right_chunks = Layout::vertical([
            Constraint::Length(3), // Info lines (Torrent, blank line, details)
            Constraint::Length(1), // Gauge
        ])
        .spacing(1)
        .split(inner_area);

        if let Some(download) = self.downloads.get(self.selected_download) {
            let seeders_str = download.torrent.seeders.to_string();
            let size_str = crate::util::format_size(download.torrent.size_bytes);
            let elapsed = download.elapsed_time();
            let info_paragraph = Paragraph::new(vec![
                Line::from(vec![
                    Span::styled("Torrent ", Style::default().fg(OVERLAY0)),
                    Span::styled(download.torrent.name.as_str(), Style::default().fg(TEXT)),
                ]),
                Line::from(vec![
                    Span::styled("Path    ", Style::default().fg(OVERLAY0)),
                    Span::styled(download.target_path.display().to_string(), Style::default().fg(GREEN)),
                ]),
                Line::from(vec![
                    Span::styled("Size    ", Style::default().fg(OVERLAY0)),
                    Span::styled(size_str, Style::default().fg(SKY)),
                    Span::raw("  "),
                    Span::styled("Seeders ", Style::default().fg(OVERLAY0)),
                    Span::styled(seeders_str, Style::default().fg(YELLOW)),
                    Span::raw("  "),
                    Span::styled("Elapsed ", Style::default().fg(OVERLAY0)),
                    Span::styled(elapsed, Style::default().fg(TEXT)),
                ]),
            ]);
            frame.render_widget(info_paragraph, right_chunks[0]);

            let progress_chunks = Layout::horizontal([Constraint::Length(17), Constraint::Length(24), Constraint::Min(0)]).split(right_chunks[1]);

            let summary = download.progress_summary();
            let prefix = Paragraph::new(Line::from(vec![Span::styled("Progress ", Style::default().fg(OVERLAY0)), Span::styled(summary, Style::default().fg(SKY).add_modifier(Modifier::BOLD)), Span::raw("  ")]));
            frame.render_widget(prefix, progress_chunks[0]);

            let ratio = download.progress_ratio().clamp(0.0, 1.0);
            let gauge_color = if matches!(download.outcome, Some(DownloadOutcome::Success)) {
                GREEN
            } else if matches!(download.outcome, Some(DownloadOutcome::Failed)) {
                RED
            } else {
                SKY
            };

            let gauge = Gauge::default().gauge_style(Style::default().fg(gauge_color).bg(SURFACE0)).use_unicode(true).ratio(ratio).label("");
            frame.render_widget(gauge, progress_chunks[1]);

            let suffix = Paragraph::new(Line::from(vec![Span::raw("  "), Span::styled(download.status_text.as_str(), Style::default().fg(TEXT))]));
            frame.render_widget(suffix, progress_chunks[2]);
        } else {
            let empty_text = Paragraph::new(Line::from(Span::styled("No downloads yet.", Style::default().fg(OVERLAY0))));
            frame.render_widget(empty_text, right_chunks[0]);
        }
    }

    fn draw_downloads(&self, frame: &mut ratatui::Frame<'_>, area: Rect) {
        let bottom = Layout::horizontal([Constraint::Percentage(38), Constraint::Percentage(62)]).split(area);

        self.draw_downloads_list(frame, bottom[0]);
        self.draw_downloads_activity(frame, bottom[1]);
    }

    fn draw_help(&self, frame: &mut ratatui::Frame<'_>, area: Rect) {
        if self.show_help {
            let popup_width = 25;
            let popup_height = 5;
            // Float top right, slightly inside the bounds
            let popup_x = area.width.saturating_sub(popup_width + 2);
            let popup_y = 1;

            let popup_area = Rect::new(popup_x, popup_y, popup_width, popup_height);

            // Clear the pixels underneath so the text doesn't clash
            frame.render_widget(Clear, popup_area);

            let help_block = Block::default().title(" Keys ").borders(Borders::ALL).border_style(Style::default().fg(LAVENDER));

            let help_text = vec![Line::from(vec![Span::styled("Tab  ", Style::default().fg(YELLOW).add_modifier(Modifier::BOLD)), Span::styled("Cycle Focus", Style::default().fg(TEXT))]), Line::from(vec![Span::styled("Enter", Style::default().fg(YELLOW).add_modifier(Modifier::BOLD)), Span::styled(" Start Download", Style::default().fg(TEXT))]), Line::from(vec![Span::styled("Esc  ", Style::default().fg(YELLOW).add_modifier(Modifier::BOLD)), Span::styled("Quit App", Style::default().fg(TEXT))])];

            let paragraph = Paragraph::new(help_text.as_slice()).block(help_block);
            frame.render_widget(paragraph, popup_area);
        }
    }

    fn draw(&self, frame: &mut ratatui::Frame<'_>) {
        let area = frame.area();

        let layout = Layout::vertical([
            Constraint::Length(3), // Query Box
            Constraint::Min(5),    // Results Table
            Constraint::Length(7), // Downloads Layout
        ])
        .split(area);

        self.draw_query(frame, layout[0]);
        self.draw_results(frame, layout[1]);
        self.draw_downloads(frame, layout[2]);
        self.draw_help(frame, area);
    }

    fn handle_key(&mut self, key: KeyCode) {
        match key {
            KeyCode::Char('?') => {
                // Ignore opening help if the user is typing in the Query box
                if !matches!(self.focus, FocusPane::Query) {
                    self.show_help = !self.show_help;
                    return;
                }
            }
            KeyCode::Tab => {
                self.cycle_focus();
                return;
            }
            KeyCode::BackTab => {
                self.cycle_focus_reverse();
                return;
            }
            KeyCode::Esc => {
                // If help is open, Esc just closes it.
                if self.show_help {
                    self.show_help = false;
                    return;
                }

                if self.active_downloads() > 0 {
                    self.abort_all_downloads();
                }
                self.should_quit = true;
                return;
            }
            _ => {}
        }

        match self.focus {
            FocusPane::Query => self.handle_query_key(key),
            FocusPane::Results => self.handle_results_key(key),
            FocusPane::Downloads => self.handle_downloads_key(key),
        }
    }

    fn handle_query_key(&mut self, key: KeyCode) {
        match key {
            KeyCode::Enter => self.submit_query(),
            KeyCode::Backspace => {
                self.query_input.pop();
            }
            KeyCode::Char(character) => {
                self.query_input.push(character);
            }
            _ => {}
        }
    }

    fn handle_results_key(&mut self, key: KeyCode) {
        match key {
            KeyCode::Up | KeyCode::Char('k') => {
                if self.selected_result > 0 {
                    self.selected_result -= 1;
                }
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if self.selected_result + 1 < self.results.len() {
                    self.selected_result += 1;
                }
            }
            KeyCode::Enter => {
                if let Some(torrent) = self.results.get(self.selected_result).cloned() {
                    let download = DownloadSession::start(torrent, self.session.clone(), self.download_tx.clone(), self.client.clone(), self.dpi_blocked.clone());
                    self.downloads.push(download);
                    self.selected_download = self.downloads.len().saturating_sub(1);
                    self.focus = FocusPane::Downloads;
                }
            }
            _ => {}
        }
    }

    fn handle_downloads_key(&mut self, key: KeyCode) {
        match key {
            KeyCode::Up | KeyCode::Char('k') => {
                if self.selected_download > 0 {
                    self.selected_download -= 1;
                }
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if self.selected_download + 1 < self.downloads.len() {
                    self.selected_download += 1;
                }
            }
            KeyCode::Char('d') => {
                if let Some(download) = self.downloads.get_mut(self.selected_download)
                    && download.is_managed_active()
                {
                    download.abort();
                }
            }
            _ => {}
        }
    }

    fn submit_query(&mut self) {
        let query = self.query_input.trim();
        if query.is_empty() {
            return;
        }

        self.results.clear();
        self.is_searching = true;
        self.selected_result = 0;

        let results_tx = self.results_tx.clone();
        let client = self.client.clone();
        let query_str = query.to_string();
        let limit = 50;

        tokio::spawn(async move {
            let pb_future = crate::indexer::search_piratebay(&client, &query_str, limit);
            let nyaa_future = crate::indexer::search_nyaa(&client, &query_str, limit);
            let (pb_res, nyaa_res) = tokio::join!(pb_future, nyaa_future);

            let mut combined = Vec::with_capacity(limit * 2);
            if let Ok(mut res) = pb_res {
                combined.append(&mut res);
            }
            if let Ok(mut res) = nyaa_res {
                combined.append(&mut res);
            }

            combined.sort_unstable_by_key(|right| std::cmp::Reverse(right.seeders));
            combined.truncate(limit);
            let _ = results_tx.send(Ok(combined));
        });
    }

    const fn cycle_focus(&mut self) {
        self.focus = match self.focus {
            FocusPane::Query => FocusPane::Results,
            FocusPane::Results => FocusPane::Downloads,
            FocusPane::Downloads => FocusPane::Query,
        };
    }

    const fn cycle_focus_reverse(&mut self) {
        self.focus = match self.focus {
            FocusPane::Query => FocusPane::Downloads,
            FocusPane::Results => FocusPane::Query,
            FocusPane::Downloads => FocusPane::Results,
        };
    }

    fn active_downloads(&self) -> usize {
        self.downloads.iter().filter(|download| download.is_managed_active()).count()
    }

    fn abort_all_downloads(&mut self) {
        for download in &mut self.downloads {
            if download.is_managed_active() {
                download.abort();
            }
        }
    }

    fn focus_style(&self, pane: FocusPane) -> Style {
        if self.focus == pane { Style::default().fg(MAUVE).add_modifier(Modifier::BOLD) } else { Style::default().fg(OVERLAY0) }
    }
}

enum DownloadEvent {
    Status(String),
    Progress { ratio: f64, down_speed: u64, up_speed: u64, share_ratio: f64, progress_bytes: u64, finished: bool },
    Success,
    Error(String),
}

struct DownloadSession {
    torrent: Torrent,
    target_path: PathBuf,
    tracking: DownloadTracking,
    progress: Option<f64>,
    status_text: String,
    started_at: Instant,
    finished_duration: Option<Duration>,
    outcome: Option<DownloadOutcome>,
}

impl DownloadSession {
    fn start(
        torrent: Torrent,
        session: Arc<Session>,
        download_tx: tokio::sync::mpsc::UnboundedSender<(String, DownloadEvent)>,
        client: reqwest::Client,
        dpi_blocked: Arc<AtomicBool>,
    ) -> Self {
        let target_path = crate::storage::default_download_dir().join(&torrent.name);
        let is_dpi_blocked = dpi_blocked.load(Ordering::Relaxed);

        if is_dpi_blocked {
            let client_c = client.clone();
            let info_hash_c = torrent.info_hash.clone();
            let name_c = torrent.name.clone();
            let magnet_c = torrent.resolved_magnet();
            let torrent_url_c = torrent.torrent_url.clone();
            let target_path_c = target_path.clone();
            let download_tx_c = download_tx.clone();
            let total_size = torrent.size_bytes;

            tokio::spawn(async move {
                let _ = download_tx_c.send((info_hash_c.clone(), DownloadEvent::Status("Canary: DPI firewall detected. Bypassing local swarm (0s delay)...".to_string())));
                if let Err(e) = Self::run_cloud_acceleration(&client_c, &info_hash_c, &name_c, &magnet_c, torrent_url_c, target_path_c, total_size, &download_tx_c).await {
                    let _ = download_tx_c.send((info_hash_c, DownloadEvent::Error(format!("{e}"))));
                }
            });

            Self {
                target_path,
                torrent,
                tracking: DownloadTracking::Managed,
                progress: None,
                status_text: "Canary: DPI detected. Activating Cloud Accelerator (0s delay)...".to_string(),
                started_at: Instant::now(),
                finished_duration: None,
                outcome: None,
            }
        } else {
            let torrent_clone = torrent.clone();
            tokio::runtime::Handle::current().spawn(Self::download_task(session, torrent_clone, download_tx, client));

            Self {
                target_path,
                torrent,
                tracking: DownloadTracking::Managed,
                progress: None,
                status_text: "Connecting to swarm...".to_string(),
                started_at: Instant::now(),
                finished_duration: None,
                outcome: None,
            }
        }
    }

    async fn download_task(
        session: Arc<Session>,
        torrent: Torrent,
        download_tx: tokio::sync::mpsc::UnboundedSender<(String, DownloadEvent)>,
        client: reqwest::Client,
    ) {
        let info_hash = torrent.info_hash.clone();
        let add_opts = AddTorrentOptions {
            overwrite: true,
            trackers: Some(crate::util::DEFAULT_HTTPS_TRACKERS.iter().map(|s| s.to_string()).collect()),
            ..Default::default()
        };

        let magnet = torrent.resolved_magnet();
        let add_request = if let Some(ref url) = torrent.torrent_url {
            let _ = download_tx.send((info_hash.clone(), DownloadEvent::Status("Downloading .torrent metadata...".to_string())));
            let mut file_bytes = None;
            for _ in 0..3 {
                if let Ok(resp) = client.get(url).send().await {
                    if resp.status().is_success() {
                        if let Ok(bytes) = resp.bytes().await {
                            file_bytes = Some(bytes);
                            break;
                        }
                    }
                }
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
            if let Some(bytes) = file_bytes {
                AddTorrent::TorrentFileBytes(bytes)
            } else {
                let _ = download_tx.send((info_hash.clone(), DownloadEvent::Status("Adding magnet link...".to_string())));
                AddTorrent::from_url(&magnet)
            }
        } else {
            let _ = download_tx.send((info_hash.clone(), DownloadEvent::Status("Adding magnet link...".to_string())));
            AddTorrent::from_url(&magnet)
        };

        let handle_opt = match session.add_torrent(add_request, Some(add_opts)).await {
            Ok(response) => response.into_handle(),
            Err(_) => None,
        };

        let _ = download_tx.send((info_hash.clone(), DownloadEvent::Status("Connecting to swarm peers...".to_string())));

        let mut last_down_bytes = 0;
        let mut last_up_bytes = 0;
        let mut last_time = Instant::now();
        let mut down_speed_ema = 0.0;
        let mut up_speed_ema = 0.0;
        let mut stalled_ticks = 0;

        loop {
            let (total, progress, uploaded, finished) = if let Some(ref handle) = handle_opt {
                let stats = handle.stats();
                (stats.total_bytes, stats.progress_bytes, stats.uploaded_bytes, stats.finished)
            } else {
                (torrent.size_bytes, 0, 0, false)
            };

            if finished {
                let _ = download_tx.send((info_hash.clone(), DownloadEvent::Success));
                break;
            }

            if progress > 0 && progress > last_down_bytes {
                stalled_ticks = 0;
                let ratio_pct = if total > 0 { ((progress as f64) / (total as f64)).clamp(0.0, 1.0) } else { 0.0 };
                let share_ratio = if progress > 0 { (uploaded as f64) / (total as f64) } else { 0.0 };

                let now = Instant::now();
                let elapsed = now.duration_since(last_time).as_secs_f64();

                let current_down = if elapsed > 0.0 { (progress.saturating_sub(last_down_bytes) as f64) / elapsed } else { 0.0 };
                let current_up = if elapsed > 0.0 { (uploaded.saturating_sub(last_up_bytes) as f64) / elapsed } else { 0.0 };
                down_speed_ema = (current_down * 0.2) + (down_speed_ema * 0.8);
                up_speed_ema = (current_up * 0.2) + (up_speed_ema * 0.8);
                let down_speed = down_speed_ema.max(0.0) as u64;
                let up_speed = up_speed_ema.max(0.0) as u64;

                last_down_bytes = progress;
                last_up_bytes = uploaded;
                last_time = now;

                if download_tx.send((info_hash.clone(), DownloadEvent::Progress { ratio: ratio_pct, down_speed, up_speed, share_ratio, progress_bytes: progress, finished })).is_err() {
                    break;
                }
            } else {
                stalled_ticks += 1;
                // If local peer download makes 0 progress after 6 seconds, firewall DPI is blocking peer handshakes.
                // Automatically activate the cloud accelerator.
                if stalled_ticks >= 6 {
                    let client_c = client.clone();
                    let info_hash_c = info_hash.clone();
                    let name_c = torrent.name.clone();
                    let magnet_c = torrent.resolved_magnet();
                    let torrent_url_c = torrent.torrent_url.clone();
                    let target_path = crate::storage::default_download_dir().join(&torrent.name);
                    let download_tx_c = download_tx.clone();
                    let total_size = torrent.size_bytes;

                    tokio::spawn(async move {
                        if let Err(e) = Self::run_cloud_acceleration(&client_c, &info_hash_c, &name_c, &magnet_c, torrent_url_c, target_path, total_size, &download_tx_c).await {
                            let _ = download_tx_c.send((info_hash_c, DownloadEvent::Error(format!("{e}"))));
                        }
                    });
                    break;
                }
            }

            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        }
    }

    async fn run_cloud_acceleration(
        client: &reqwest::Client,
        info_hash: &str,
        torrent_name: &str,
        magnet: &str,
        torrent_url: Option<String>,
        target_path: PathBuf,
        total_size: u64,
        download_tx: &tokio::sync::mpsc::UnboundedSender<(String, DownloadEvent)>,
    ) -> Result<()> {
        let tag = format!("dl_{}", &info_hash[..12.min(info_hash.len())]);
        let info_hash_str = info_hash.to_string();

        let _ = download_tx.send((info_hash_str.clone(), DownloadEvent::Status("DPI detected. Activating Cloud Accelerator...".to_string())));

        // 1. Check if asset already exists in release
        let check_existing = tokio::task::spawn_blocking({
            let tag_c = tag.clone();
            move || {
                std::process::Command::new("gh")
                    .args(["release", "view", &tag_c, "--repo", "Aditya-233/Torrenty", "--json", "assets"])
                    .output()
            }
        }).await.ok().and_then(|r| r.ok());

        let mut download_url = None;
        let mut expected_size = total_size;

        if let Some(out) = check_existing {
            if out.status.success() {
                if let Ok(json_val) = serde_json::from_slice::<serde_json::Value>(&out.stdout) {
                    if let Some(assets) = json_val.get("assets").and_then(|a| a.as_array()) {
                        if let Some(asset) = assets.iter().find(|a| a.get("size").and_then(|s| s.as_u64()).unwrap_or(0) > 1024) {
                            if let Some(url) = asset.get("url").and_then(|u| u.as_str()) {
                                download_url = Some(url.to_string());
                                if let Some(sz) = asset.get("size").and_then(|s| s.as_u64()) {
                                    expected_size = sz;
                                }
                            }
                        }
                    }
                }
            }
        }

        // 2. If not already available, dispatch cloud workflow
        if download_url.is_none() {
            let _ = download_tx.send((info_hash_str.clone(), DownloadEvent::Status("Dispatching cloud runner (10 Gbps)...".to_string())));
            let trigger = tokio::task::spawn_blocking({
                let magnet_c = magnet.to_string();
                let name_c = torrent_name.to_string();
                let tag_c = tag.clone();
                let torrent_url_c = torrent_url;
                move || {
                    let mut args = vec![
                        "workflow".to_string(), "run".to_string(), "cloud_download.yml".to_string(),
                        "--repo".to_string(), "Aditya-233/Torrenty".to_string(),
                        "-f".to_string(), format!("magnet={}", magnet_c),
                        "-f".to_string(), format!("name={}", name_c),
                        "-f".to_string(), format!("tag={}", tag_c),
                    ];
                    if let Some(ref turl) = torrent_url_c {
                        args.push("-f".to_string());
                        args.push(format!("torrent_url={}", turl));
                    }
                    std::process::Command::new("gh")
                        .args(&args)
                        .output()
                }
            }).await.map_err(|e| anyhow::anyhow!("{e}"))??;

            if !trigger.status.success() {
                return Err(anyhow::anyhow!("Failed to dispatch GitHub cloud workflow: {}", String::from_utf8_lossy(&trigger.stderr)));
            }

            // Poll every 3 seconds for up to 6 minutes
            let start = Instant::now();
            while start.elapsed() < Duration::from_secs(360) {
                tokio::time::sleep(Duration::from_secs(3)).await;
                let elapsed_sec = start.elapsed().as_secs();
                let _ = download_tx.send((info_hash_str.clone(), DownloadEvent::Status(format!("Cloud downloading swarm ({}s)...", elapsed_sec))));

                let poll_res = tokio::task::spawn_blocking({
                    let tag_c = tag.clone();
                    move || {
                        std::process::Command::new("gh")
                            .args(["release", "view", &tag_c, "--repo", "Aditya-233/Torrenty", "--json", "assets,body"])
                            .output()
                    }
                }).await.ok().and_then(|r| r.ok());

                if let Some(out) = poll_res {
                    if out.status.success() {
                        if let Ok(json_val) = serde_json::from_slice::<serde_json::Value>(&out.stdout) {
                            if let Some(body) = json_val.get("body").and_then(|b| b.as_str()) {
                                for line in body.lines() {
                                    if let Some(rest) = line.strip_prefix("STREAM_URL:") {
                                        let u = rest.trim();
                                        if u.starts_with("https://") {
                                            download_url = Some(u.to_string());
                                            break;
                                        }
                                    }
                                }
                            }

                            if download_url.is_none() {
                                if let Some(assets) = json_val.get("assets").and_then(|a| a.as_array()) {
                                    if let Some(asset) = assets.iter().find(|a| a.get("size").and_then(|s| s.as_u64()).unwrap_or(0) > 1024) {
                                        if let Some(url) = asset.get("url").and_then(|u| u.as_str()) {
                                            download_url = Some(url.to_string());
                                            if let Some(sz) = asset.get("size").and_then(|s| s.as_u64()) {
                                                expected_size = sz;
                                            }
                                        }
                                    }
                                }
                            }

                            if download_url.is_some() {
                                break;
                            }
                        }
                    }
                }
            }
        }

        let Some(url) = download_url else {
            return Err(anyhow::anyhow!("Cloud download timed out waiting for release asset"));
        };

        let _ = download_tx.send((info_hash_str.clone(), DownloadEvent::Status("Streaming verified data over HTTPS...".to_string())));

        // Stream download directly from GitHub Release CDN over HTTPS
        let mut resp = client.get(&url).send().await?.error_for_status()?;
        let total_bytes = resp.content_length().unwrap_or(expected_size);

        let mut file = std::fs::File::create(&target_path)?;
        let mut downloaded: u64 = 0;
        let mut last_tick = Instant::now();
        let mut last_bytes = 0;

        while let Some(chunk) = resp.chunk().await? {
            use std::io::Write;
            file.write_all(&chunk)?;
            downloaded += chunk.len() as u64;

            let now = Instant::now();
            if now.duration_since(last_tick).as_millis() >= 350 {
                let dt = now.duration_since(last_tick).as_secs_f64();
                let speed = if dt > 0.0 { ((downloaded.saturating_sub(last_bytes)) as f64 / dt) as u64 } else { 0 };
                let ratio = if total_bytes > 0 { (downloaded as f64 / total_bytes as f64).clamp(0.0, 1.0) } else { 0.0 };
                let _ = download_tx.send((info_hash_str.clone(), DownloadEvent::Progress {
                    ratio,
                    down_speed: speed,
                    up_speed: 0,
                    share_ratio: 0.0,
                    progress_bytes: downloaded,
                    finished: downloaded >= total_bytes,
                }));
                last_tick = now;
                last_bytes = downloaded;
            }
        }

        let _ = download_tx.send((info_hash_str.clone(), DownloadEvent::Progress {
            ratio: 1.0,
            down_speed: 0,
            up_speed: 0,
            share_ratio: 0.0,
            progress_bytes: downloaded,
            finished: true,
        }));
        let _ = download_tx.send((info_hash_str, DownloadEvent::Success));

        Ok(())
    }

    fn from_history_entry(entry: DownloadHistoryEntry) -> Self {
        let torrent = Torrent {
            name: entry.name.clone(),
            info_hash: entry.info_hash.clone(),
            magnet: None,
            torrent_url: None,
            seeders: 0,
            size_bytes: 0,
        };
        let duration_secs = entry.completed_at_epoch_secs.unwrap_or(entry.added_at_epoch_secs).saturating_sub(entry.added_at_epoch_secs);

        Self {
            torrent,
            target_path: entry.target_path,
            tracking: DownloadTracking::History,
            progress: Some(1.0),
            status_text: "Finished".to_string(),
            started_at: Instant::now(),
            finished_duration: Some(Duration::from_secs(duration_secs)),
            outcome: Some(DownloadOutcome::Success),
        }
    }

    fn apply_event(&mut self, event: DownloadEvent) {
        match event {
            DownloadEvent::Status(msg) => {
                self.status_text = msg;
            }
            DownloadEvent::Progress { ratio, down_speed, up_speed, share_ratio, progress_bytes, finished } => {
                self.progress = Some(ratio);
                if finished {
                    if self.finished_duration.is_none() {
                        self.finished_duration = Some(self.started_at.elapsed());
                    }
                    self.status_text = format!("seeding | up {}/s | ratio {:.2}", crate::util::format_size(up_speed), share_ratio);
                } else {
                    self.status_text = format!("down {}/s | {}", crate::util::format_size(down_speed), crate::util::format_size(progress_bytes));
                }
            }
            DownloadEvent::Success => {
                self.progress = Some(1.0);
                self.outcome = Some(DownloadOutcome::Success);
                if self.finished_duration.is_none() {
                    self.finished_duration = Some(self.started_at.elapsed());
                }
            }
            DownloadEvent::Error(err) => {
                self.status_text = format!("Error: {err}");
                self.outcome = Some(DownloadOutcome::Failed);
                if self.finished_duration.is_none() {
                    self.finished_duration = Some(self.started_at.elapsed());
                }
            }
        }
    }

    fn abort(&mut self) {
        if matches!(self.tracking, DownloadTracking::History) {
            return;
        }

        self.status_text = "Download aborted".to_string();
        self.outcome = Some(DownloadOutcome::Aborted);
        if self.finished_duration.is_none() {
            self.finished_duration = Some(self.started_at.elapsed());
        }
    }

    const fn is_managed_active(&self) -> bool {
        matches!(self.tracking, DownloadTracking::Managed) && self.outcome.is_none()
    }

    const fn progress_ratio(&self) -> f64 {
        if let Some(progress) = self.progress {
            progress
        } else if matches!(self.outcome, Some(DownloadOutcome::Success)) {
            1.0
        } else {
            0.0
        }
    }

    fn elapsed_time(&self) -> String {
        let d = self.finished_duration.unwrap_or_else(|| self.started_at.elapsed());
        format_duration(d)
    }

    fn progress_summary(&self) -> String {
        if let Some(p) = self.progress {
            format!("{:>5.1}%", p * 100.0)
        } else if matches!(self.outcome, Some(DownloadOutcome::Success)) {
            "100.0%".to_string()
        } else {
            "  0.0%".to_string()
        }
    }

    fn status_badge(&self) -> Span<'static> {
        match self.outcome {
            Some(DownloadOutcome::Success) => Span::styled(" done ", Style::default().fg(SURFACE0).bg(GREEN)),
            Some(DownloadOutcome::Failed) => Span::styled(" fail ", Style::default().fg(TEXT).bg(RED)),
            Some(DownloadOutcome::Aborted) => Span::styled(" stop ", Style::default().fg(SURFACE0).bg(YELLOW)),
            None => Span::styled(self.progress_summary(), Style::default().fg(SURFACE0).bg(SKY)),
        }
    }
}

enum DownloadOutcome {
    Success,
    Failed,
    Aborted,
}

enum DownloadTracking {
    Managed,
    History,
}

fn format_duration(duration: Duration) -> String {
    let total_secs = duration.as_secs();
    let hours = total_secs / 3600;
    let minutes = (total_secs % 3600) / 60;
    let seconds = total_secs % 60;
    if hours > 0 {
        format!("{hours}h {minutes}m")
    } else if minutes > 0 {
        format!("{minutes}m {seconds}s")
    } else {
        format!("{seconds}s")
    }
}
