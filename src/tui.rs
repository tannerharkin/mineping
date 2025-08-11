use crate::api::{get_stats_data, get_worker_states_snapshot, WorkerState};
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Wrap},
    Frame, Terminal,
};
use std::collections::VecDeque;
use std::io;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;
use tracing::Level;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

const MAX_LOG_LINES: usize = 1000;
const LOG_UPDATE_INTERVAL: Duration = Duration::from_millis(100);
const STATS_UPDATE_INTERVAL: Duration = Duration::from_millis(500);

pub struct TuiState {
    logs: Arc<RwLock<VecDeque<LogEntry>>>,
    should_quit: Arc<RwLock<bool>>,
    cached_stats: Arc<RwLock<Option<crate::api::StatsResponse>>>,
    cached_workers: Arc<RwLock<Vec<WorkerState>>>,
}

#[derive(Clone)]
struct LogEntry {
    level: Level,
    message: String,
    timestamp: chrono::DateTime<chrono::Local>,
}

impl TuiState {
    pub fn new() -> Self {
        Self {
            logs: Arc::new(RwLock::new(VecDeque::with_capacity(MAX_LOG_LINES))),
            should_quit: Arc::new(RwLock::new(false)),
            cached_stats: Arc::new(RwLock::new(None)),
            cached_workers: Arc::new(RwLock::new(Vec::new())),
        }
    }

    pub fn get_log_writer(&self) -> TuiLogWriter {
        TuiLogWriter {
            logs: Arc::clone(&self.logs),
        }
    }

    pub async fn run(self) -> Result<(), io::Error> {
        // Setup terminal
        enable_raw_mode()?;
        let mut stdout = io::stdout();
        execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
        let backend = CrosstermBackend::new(stdout);
        let mut terminal = Terminal::new(backend)?;

        // Run the app
        let res = self.run_app(&mut terminal).await;

        // Restore terminal
        disable_raw_mode()?;
        execute!(
            terminal.backend_mut(),
            LeaveAlternateScreen,
            DisableMouseCapture
        )?;
        terminal.show_cursor()?;

        res
    }

    async fn run_app(&self, terminal: &mut Terminal<CrosstermBackend<io::Stdout>>) -> Result<(), io::Error> {
        let mut stats_interval = tokio::time::interval(STATS_UPDATE_INTERVAL);
        let mut render_interval = tokio::time::interval(LOG_UPDATE_INTERVAL);

        // Spawn task to update cached data
        let stats_cache = Arc::clone(&self.cached_stats);
        let workers_cache = Arc::clone(&self.cached_workers);
        let update_handle = tokio::spawn(async move {
            let mut interval = tokio::time::interval(STATS_UPDATE_INTERVAL);
            loop {
                interval.tick().await;
                
                // Update stats
                let stats = get_stats_data().await;
                *stats_cache.write().await = Some(stats);
                
                // Update workers
                let workers = get_worker_states_snapshot().await;
                *workers_cache.write().await = workers;
            }
        });

        loop {
            // Check if we should quit
            if *self.should_quit.read().await {
                break;
            }

            tokio::select! {
                _ = stats_interval.tick() => {
                    // Stats are updated in background task
                }
                _ = render_interval.tick() => {
                    self.render_ui(terminal).await?;
                }
                _ = tokio::time::sleep(Duration::from_millis(50)) => {
                    // Check for input
                    if event::poll(Duration::from_millis(10))? {
                        if let Event::Key(key) = event::read()? {
                            match key.code {
                                KeyCode::Char('q') | KeyCode::Char('Q') | KeyCode::Esc => {
                                    *self.should_quit.write().await = true;
                                    break;
                                }
                                KeyCode::Char('c') | KeyCode::Char('C') => {
                                    if key.modifiers.contains(event::KeyModifiers::CONTROL) {
                                        *self.should_quit.write().await = true;
                                        break;
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                }
            }
        }

        // Cleanup
        update_handle.abort();
        Ok(())
    }

    async fn render_ui(&self, terminal: &mut Terminal<CrosstermBackend<io::Stdout>>) -> Result<(), io::Error> {
        let stats = self.cached_stats.read().await.clone();
        let workers = self.cached_workers.read().await.clone();
        let logs = self.logs.read().await.clone();

        terminal.draw(|frame| {
            self.ui(frame, stats, workers, logs);
        })?;
        
        Ok(())
    }

    fn ui(&self, frame: &mut Frame, stats: Option<crate::api::StatsResponse>, workers: Vec<WorkerState>, logs: VecDeque<LogEntry>) {
        let size = frame.area();
        
        // Reserve space for controls footer (1 line)
        let main_and_footer = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Min(0),
                Constraint::Length(1),
            ])
            .split(size);
        
        let main_area = main_and_footer[0];
        let footer_area = main_and_footer[1];
        
        // Determine layout based on terminal height
        let layout = if main_area.height < 40 {
            // Small terminal: split in half
            Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Percentage(50),
                    Constraint::Percentage(50),
                ])
                .split(main_area)
        } else {
            // Large terminal: logs get 2/3
            Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Percentage(33),
                    Constraint::Percentage(67),
                ])
                .split(main_area)
        };

        // Split top section into stats and workers
        let top_layout = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Percentage(50),
                Constraint::Percentage(50),
            ])
            .split(layout[0]);

        // Render components
        self.render_stats(frame, top_layout[0], stats);
        self.render_workers(frame, top_layout[1], workers);
        self.render_logs(frame, layout[1], logs);
        self.render_footer(frame, footer_area);
    }

    fn render_stats(&self, frame: &mut Frame, area: Rect, stats: Option<crate::api::StatsResponse>) {
        let lines = if let Some(stats) = stats {
            let mut lines = vec![];
            
            // Determine if we have space for full info layout
            let use_columns = area.width > 50 && area.height > 15;
            
            if use_columns {
                // Two-column layout for wider terminals
                lines.push(Line::from(vec![
                    Span::styled("Uptime: ", Style::default().fg(Color::Cyan)),
                    Span::raw(format_duration(stats.uptime_seconds)),
                ]));
                lines.push(Line::from(""));
                
                // Requests and Queue side by side
                lines.push(Line::from(vec![
                    Span::styled("Requests", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
                    Span::raw("                  "),  // More spacing for larger numbers
                    Span::styled("Queue", Style::default().fg(Color::Magenta).add_modifier(Modifier::BOLD)),
                ]));
                
                lines.push(Line::from(format!(
                    " Total: {:<18} Pending: {}",
                    stats.total_requests,
                    stats.queue_stats.pending_tickets
                )));
                
                lines.push(Line::from(format!(
                    " Success: {:<16} Processing: {}",
                    format!("{} ({:.1}%)", stats.successful_requests, stats.success_rate),
                    stats.queue_stats.processing_tickets
                )));
                
                lines.push(Line::from(format!(
                    " Failed: {:<17} Completed: {}",
                    stats.failed_requests,
                    stats.queue_stats.total_completed
                )));
                
                lines.push(Line::from(format!(
                    " Rate: {:<19} Avg Queue: {}ms",
                    format!("{:.1}/min", stats.requests_per_minute),
                    stats.queue_stats.avg_queue_time_ms
                )));
                
                lines.push(Line::from(format!(
                    "                           Avg Process: {}ms",
                    stats.queue_stats.avg_process_time_ms
                )));
                
                lines.push(Line::from(""));
                lines.push(Line::from(vec![
                    Span::styled("Protocols", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
                ]));
                lines.push(Line::from(format!(
                    " Modern: {:.1}%  Legacy: {:.1}%  Query: {:.1}%",
                    stats.protocol_stats.modern_slp_percentage,
                    stats.protocol_stats.legacy_slp_percentage,
                    stats.protocol_stats.query_enabled_percentage
                )));
            } else {
                // Compressed layout for smaller terminals
                lines.push(Line::from(vec![
                    Span::styled("Up: ", Style::default().fg(Color::Cyan)),
                    Span::raw(format_duration(stats.uptime_seconds)),
                ]));
                
                lines.push(Line::from(vec![
                    Span::styled("Req: ", Style::default().fg(Color::Yellow)),
                    Span::raw(format!("{} ({:.0}%)", stats.total_requests, stats.success_rate)),
                ]));
                
                lines.push(Line::from(vec![
                    Span::styled("Queue: ", Style::default().fg(Color::Magenta)),
                    Span::raw(format!("{}/{}", stats.queue_stats.pending_tickets, stats.queue_stats.processing_tickets)),
                ]));
                
                lines.push(Line::from(vec![
                    Span::styled("Rate: ", Style::default().fg(Color::White)),
                    Span::raw(format!("{:.1}/min", stats.requests_per_minute)),
                ]));
                
                lines.push(Line::from(vec![
                    Span::styled("Proto: ", Style::default().fg(Color::Cyan)),
                    Span::raw(format!("M:{:.0}% L:{:.0}%", 
                        stats.protocol_stats.modern_slp_percentage,
                        stats.protocol_stats.legacy_slp_percentage
                    )),
                ]));
            }
            
            lines
        } else {
            vec![
                Line::from(vec![
                    Span::styled("Loading statistics...", Style::default().fg(Color::DarkGray))
                ])
            ]
        };

        let paragraph = Paragraph::new(lines)
            .block(Block::default().borders(Borders::ALL).title(" Statistics "))
            .alignment(Alignment::Left);

        frame.render_widget(paragraph, area);
    }

    fn render_workers(&self, frame: &mut Frame, area: Rect, worker_states: Vec<WorkerState>) {
        let mut lines = vec![];
        
        // Worker threads header
        lines.push(Line::from(vec![
            Span::styled(
                format!("Workers ({}):", worker_states.len()),
                Style::default().fg(Color::Cyan)
            ),
        ]));
        
        // Calculate how many workers fit per line (accounting for padding and spacing)
        let available_width = area.width.saturating_sub(4) as usize; // 2 for padding, 2 for borders
        let workers_per_line = available_width.min(worker_states.len());
        
        // Track processing hosts for display
        let mut processing_hosts: Vec<String> = Vec::new();
        
        // Create worker blocks
        if !worker_states.is_empty() {
            let mut current_line_blocks = Vec::new();
            let mut current_line_count = 0;
            
            for state in worker_states.iter() {
                // Determine block character and color for each worker individually
                let (block, color) = match state {
                    WorkerState::Idle => ('▓', Color::DarkGray),
                    WorkerState::Processing { host } => {
                        processing_hosts.push(host.clone());
                        ('█', Color::Green)
                    },
                    WorkerState::Connecting => ('█', Color::Yellow),
                };
                
                // Add the colored block
                current_line_blocks.push(Span::styled(block.to_string(), Style::default().fg(color)));
                current_line_count += 1;
                
                // Check if we need to wrap to next line
                if current_line_count >= workers_per_line {
                    let mut line_spans = vec![Span::raw(" ")];
                    line_spans.extend(current_line_blocks.clone());
                    lines.push(Line::from(line_spans));
                    current_line_blocks.clear();
                    current_line_count = 0;
                }
            }
            
            // Add any remaining blocks
            if !current_line_blocks.is_empty() {
                let mut line_spans = vec![Span::raw(" ")];
                line_spans.extend(current_line_blocks);
                lines.push(Line::from(line_spans));
            }
        }
        
        lines.push(Line::from(""));
        
        // Show currently processing hosts if any
        if !processing_hosts.is_empty() {
            lines.push(Line::from(vec![
                Span::styled("Processing:", Style::default().fg(Color::White)),
            ]));
            
            // Show more hosts if we have vertical space
            let max_hosts_to_show = if area.height > 10 { 5 } else { 3 };
            
            for host in processing_hosts.iter().take(max_hosts_to_show) {
                lines.push(Line::from(vec![
                    Span::raw(" • "),
                    Span::styled(host, Style::default().fg(Color::Green)),
                ]));
            }
            if processing_hosts.len() > max_hosts_to_show {
                lines.push(Line::from(vec![
                    Span::raw(" "),
                    Span::styled(
                        format!("... and {} more", processing_hosts.len() - max_hosts_to_show),
                        Style::default().fg(Color::DarkGray)
                    ),
                ]));
            }
            lines.push(Line::from(""));
        }
        
        // Legend - compact for small terminals
        if area.height > 8 {
            lines.push(Line::from(vec![
                Span::styled("Legend: ", Style::default().fg(Color::White)),
                Span::styled("▓", Style::default().fg(Color::DarkGray)),
                Span::raw(" Idle  "),
                Span::styled("█", Style::default().fg(Color::Green)),
                Span::raw(" Working  "),
                Span::styled("█", Style::default().fg(Color::Yellow)),
                Span::raw(" Connecting"),
            ]));
        }

        let paragraph = Paragraph::new(lines)
            .block(Block::default().borders(Borders::ALL).title(" Worker Status "))
            .alignment(Alignment::Left);

        frame.render_widget(paragraph, area);
    }

    fn render_logs(&self, frame: &mut Frame, area: Rect, logs: VecDeque<LogEntry>) {
        let mut lines: Vec<Line> = Vec::new();
        for entry in logs.iter() {
            let color = match entry.level {
                Level::ERROR => Color::Red,
                Level::WARN => Color::Yellow,
                Level::INFO => Color::Green,
                Level::DEBUG => Color::Cyan,
                Level::TRACE => Color::DarkGray,
            };
            
            let timestamp = entry.timestamp.format("%H:%M:%S%.3f");
            lines.push(Line::from(vec![
                Span::styled(
                    format!("[{}]", timestamp),
                    Style::default().fg(Color::DarkGray)
                ),
                Span::raw(" "),
                Span::styled(
                    format!("{:5}", entry.level),
                    Style::default().fg(color)
                ),
                Span::raw(" "),
                Span::raw(&entry.message),
            ]));
        }
        
        // Ensure we have at least one line to avoid panic
        if lines.is_empty() {
            lines.push(Line::from(vec![
                Span::styled("Waiting for logs...", Style::default().fg(Color::DarkGray))
            ]));
        }

        let paragraph = Paragraph::new(lines)
            .block(Block::default().borders(Borders::ALL).title(" Logs "))
            .alignment(Alignment::Left)
            .wrap(Wrap { trim: false })
            .scroll((
                logs.len().saturating_sub(area.height as usize) as u16,
                0
            ));

        frame.render_widget(paragraph, area);
    }

    fn render_footer(&self, frame: &mut Frame, area: Rect) {
        let controls = Line::from(vec![
            Span::styled(" Q", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
            Span::raw(" Quit  "),
            Span::styled("^C", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
            Span::raw(" Quit  "),
            Span::styled("ESC", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
            Span::raw(" Quit  "),
            Span::raw("│ mineping "),
            Span::styled(env!("CARGO_PKG_VERSION"), Style::default().fg(Color::DarkGray)),
        ]);

        let paragraph = Paragraph::new(vec![controls])
            .style(Style::default().bg(Color::Black))
            .alignment(Alignment::Left);

        frame.render_widget(paragraph, area);
    }

    pub fn should_quit(&self) -> Arc<RwLock<bool>> {
        Arc::clone(&self.should_quit)
    }
}

fn format_duration(seconds: u64) -> String {
    let hours = seconds / 3600;
    let minutes = (seconds % 3600) / 60;
    let secs = seconds % 60;
    
    if hours > 0 {
        format!("{}h {}m {}s", hours, minutes, secs)
    } else if minutes > 0 {
        format!("{}m {}s", minutes, secs)
    } else {
        format!("{}s", secs)
    }
}

pub struct TuiLogWriter {
    logs: Arc<RwLock<VecDeque<LogEntry>>>,
}

impl<S> tracing_subscriber::Layer<S> for TuiLogWriter
where
    S: tracing::Subscriber,
{
    fn on_event(
        &self,
        event: &tracing::Event<'_>,
        _ctx: tracing_subscriber::layer::Context<'_, S>,
    ) {
        let level = *event.metadata().level();
        let mut visitor = LogVisitor::default();
        event.record(&mut visitor);
        
        if let Some(message) = visitor.message {
            let logs_clone = Arc::clone(&self.logs);
            tokio::spawn(async move {
                let mut logs = logs_clone.write().await;
                
                while logs.len() >= MAX_LOG_LINES {
                    logs.pop_front();
                }
                
                logs.push_back(LogEntry {
                    level,
                    message,
                    timestamp: chrono::Local::now(),
                });
            });
        }
    }
}

#[derive(Default)]
struct LogVisitor {
    message: Option<String>,
}

impl tracing::field::Visit for LogVisitor {
    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        if field.name() == "message" {
            self.message = Some(value.to_string());
        }
    }

    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            self.message = Some(format!("{:?}", value));
        }
    }
}

pub fn init_tui_logging(tui_state: &TuiState) {
    let tui_layer = tui_state.get_log_writer();
    
    tracing_subscriber::registry()
        .with(tui_layer)
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"))
        )
        .init();
}