use ratatui::{
    layout::Alignment,
    prelude::{Constraint, Layout, Rect},
    style::{Color, Style, Modifier},
    text::{self, Span},
    widgets::{Block, BorderType, Borders, Paragraph, Tabs, Gauge, Sparkline},
    Frame, symbols,
};

use crate::app::App;

/// Renders the user interface widgets.
pub fn render(app: &mut App, frame: &mut Frame) {
    // This is where you add new widgets.
    // See the following resources:
    // - https://docs.rs/ratatui/latest/ratatui/widgets/index.html
    // - https://github.com/ratatui-org/ratatui/tree/master/examples
    // frame.render_widget(
    //     Paragraph::new(format!(
    //         "This is a tui template.\n\
    //             Press `Esc`, `Ctrl-C` or `q` to stop running.\n\
    //             Press left and right to increment and decrement the counter respectively.\n\
    //             Counter: {}",
    //         app.counter
    //     ))
    //     .block(
    //         Block::default()
    //             .title("Template")
    //             .title_alignment(Alignment::Center)
    //             .borders(Borders::ALL)
    //             .border_type(BorderType::Rounded),
    //     )
    //     .style(Style::default().fg(Color::Cyan).bg(Color::Black))
    //     .alignment(Alignment::Center),
    //     frame.size(),
    // )

    let chunks = Layout::default()
        .constraints([Constraint::Length(3), Constraint::Min(0)])
        .split(frame.size());
    let titles = app
        .tabs
        .titles
        .iter()
        .map(|t| text::Line::from(Span::styled(*t, Style::default().fg(Color::Green))))
        .collect();
    let tabs = Tabs::new(titles)
        .block(Block::default().borders(Borders::ALL).title(app.title))
        .highlight_style(Style::default().fg(Color::Yellow))
        .select(app.tabs.index);
    frame.render_widget(tabs, chunks[0]);

    match app.tabs.index{
        0 => draw_first_tab(frame, app, chunks[1]),
        _ => {}
    }
}

fn draw_first_tab(f: &mut Frame, app: &mut App, area: Rect) {
    let chunks = Layout::default()
        .constraints([
            Constraint::Length(9),
            Constraint::Min(8),
            Constraint::Length(7),
        ])
        .split(area);
    draw_memory_logging_speed_gauges(f, app, chunks[0]);
    // draw_charts(f, app, chunks[1]);
    // draw_text(f, chunks[2]);
}

/// 绘制内存日志产生数量的图表
fn draw_memory_logging_speed_gauges(f: &mut Frame, app: &mut App, area: Rect){
    let chunks = Layout::default()
        .constraints([
            Constraint::Length(6),
            Constraint::Length(3),
        ])
        .margin(1)
        .split(area);
    let block = Block::default().borders(Borders::ALL).title("Graphs");
    f.render_widget(block, area);

    

    let sparkline = Sparkline::default()
        .block(Block::default().title("Memory Log Speed:"))
        .style(Style::default().fg(Color::Green))
        .data(&app.memory_log_sparkline.points)
        .bar_set(if app.enhanced_graphics {
            symbols::bar::NINE_LEVELS
        } else {
            symbols::bar::THREE_LEVELS
        });
    f.render_widget(sparkline, chunks[0]);

}
