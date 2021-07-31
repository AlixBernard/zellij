//! The layout system.
//  Layouts have been moved from [`zellij-server`] to
//  [`zellij-utils`] in order to provide more helpful
//  error messages to the user until a more general
//  logging system is in place.
//  In case there is a logging system in place evaluate,
//  if [`zellij-utils`], or [`zellij-server`] is a proper
//  place.
//  If plugins should be able to depend on the layout system
//  then [`zellij-utils`] could be a proper place.
use crate::{
    input::{command::RunCommand, config::ConfigError},
    pane_size::{Dimension, PaneGeom},
    setup,
};
use crate::{serde, serde_yaml};

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::{fs::File, io::prelude::*};

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(crate = "self::serde")]
pub enum Direction {
    Horizontal,
    Vertical,
}

#[derive(Debug, Serialize, Deserialize, Clone, Copy)]
#[serde(crate = "self::serde")]
pub enum SplitSize {
    Percent(u8), // 1 to 100
    Fixed(u16),  // An absolute number of columns or rows
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(crate = "self::serde")]
pub enum Run {
    #[serde(rename = "plugin")]
    Plugin(Option<PathBuf>),
    #[serde(rename = "command")]
    Command(RunCommand),
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(crate = "self::serde")]
pub struct Layout {
    pub direction: Direction,
    #[serde(default)]
    pub parts: Vec<Layout>,
    pub split_size: Option<SplitSize>,
    pub run: Option<Run>,
}

type LayoutResult = Result<Layout, ConfigError>;

impl Layout {
    pub fn new(layout_path: &Path) -> LayoutResult {
        let mut layout_file = File::open(&layout_path)
            .or_else(|_| File::open(&layout_path.with_extension("yaml")))
            .map_err(|e| ConfigError::IoPath(e, layout_path.into()))?;

        let mut layout = String::new();
        layout_file.read_to_string(&mut layout)?;
        let layout: Layout = serde_yaml::from_str(&layout)?;
        Ok(layout)
    }

    // It wants to use Path here, but that doesn't compile.
    #[allow(clippy::ptr_arg)]
    pub fn from_dir(layout: &PathBuf, layout_dir: Option<&PathBuf>) -> LayoutResult {
        match layout_dir {
            Some(dir) => Self::new(&dir.join(layout))
                .or_else(|_| Self::from_default_assets(layout.as_path())),
            None => Self::from_default_assets(layout.as_path()),
        }
    }

    pub fn from_path_or_default(
        layout: Option<&PathBuf>,
        layout_path: Option<&PathBuf>,
        layout_dir: Option<PathBuf>,
    ) -> Option<Result<Layout, ConfigError>> {
        layout
            .map(|p| Layout::from_dir(p, layout_dir.as_ref()))
            .or_else(|| layout_path.map(|p| Layout::new(p)))
            .or_else(|| {
                Some(Layout::from_dir(
                    &std::path::PathBuf::from("default"),
                    layout_dir.as_ref(),
                ))
            })
    }

    // Currently still needed but on nightly
    // this is already possible:
    // HashMap<&'static str, Vec<u8>>
    pub fn from_default_assets(path: &Path) -> LayoutResult {
        match path.to_str() {
            Some("default") => Self::default_from_assets(),
            Some("strider") => Self::strider_from_assets(),
            Some("disable-status-bar") => Self::disable_status_from_assets(),
            None | Some(_) => Err(ConfigError::IoPath(
                std::io::Error::new(std::io::ErrorKind::Other, "The layout was not found"),
                path.into(),
            )),
        }
    }

    // TODO Deserialize the assets from bytes &[u8],
    // once serde-yaml supports zero-copy
    pub fn default_from_assets() -> LayoutResult {
        let layout: Layout =
            serde_yaml::from_str(String::from_utf8(setup::DEFAULT_LAYOUT.to_vec())?.as_str())?;
        Ok(layout)
    }

    pub fn strider_from_assets() -> LayoutResult {
        let layout: Layout =
            serde_yaml::from_str(String::from_utf8(setup::STRIDER_LAYOUT.to_vec())?.as_str())?;
        Ok(layout)
    }

    pub fn disable_status_from_assets() -> LayoutResult {
        let layout: Layout =
            serde_yaml::from_str(String::from_utf8(setup::NO_STATUS_LAYOUT.to_vec())?.as_str())?;
        Ok(layout)
    }

    pub fn total_terminal_panes(&self) -> usize {
        let mut total_panes = 0;
        total_panes += self.parts.len();
        for part in self.parts.iter() {
            match part.run {
                Some(Run::Command(_)) | None => {
                    total_panes += part.total_terminal_panes();
                }
                Some(Run::Plugin(_)) => {}
            }
        }
        total_panes
    }

    pub fn extract_run_instructions(&self) -> Vec<Option<Run>> {
        let mut run_instructions = vec![];
        if self.parts.is_empty() {
            run_instructions.push(self.run.clone());
        }
        for part in self.parts.iter() {
            let mut current_runnables = part.extract_run_instructions();
            run_instructions.append(&mut current_runnables);
        }
        run_instructions
    }

    pub fn position_panes_in_space(
        &self,
        space: &PaneGeom,
    ) -> Vec<(Layout, PaneGeom)> {
        split_space(space, self)
    }
}

fn split_space_to_parts_vertically(
    space_to_split: &PaneGeom,
    sizes: Vec<Option<SplitSize>>,
) -> Vec<PaneGeom> {
    let mut split_parts = Vec::new();
    let mut current_x_position = space_to_split.x;

    let mut parts_to_grow = Vec::new();

    // First fit in the parameterized sizes
    for size in sizes {
        let cols = match size {
            Some(SplitSize::Percent(percent)) => Dimension::percent(percent as f64),
            Some(SplitSize::Fixed(size)) => Dimension::fixed(size as usize),
            None => {
                parts_to_grow.push(current_x_position);
                // FIXME: Should be a percent constraint calculated from the number of parts
                Dimension::percent(100.0) // This is grown later on
            }
        };
        split_parts.push(PaneGeom {
            x: current_x_position,
            y: space_to_split.y,
            // FIXME: This is likely wrong and percent should be considered!
            cols,
            rows: Dimension::percent(50.0),
        });
        // FIXME: What exactly is happening here? I should only need to preserve the ordering of
        // panes
        let columns = 1;
        current_x_position += columns + 1 + 1; // 1 for gap
    }

    /*
    let mut last_flexible_index = split_parts.len() - 1;
    if let Some(new_columns) = (max_width - current_width).checked_div(parts_to_grow.len()) {
        current_width = 0;
        current_x_position = 0;
        for (idx, part) in split_parts.iter_mut().enumerate() {
            part.x = current_x_position;
            if parts_to_grow.contains(&part.x) {
                part.cols = Dimension::fixed(new_columns);
                last_flexible_index = idx;
            }
            current_width += part.cols.as_usize();
            current_x_position += part.cols.as_usize() + 1; // 1 for gap
        }
    }
    */
    /*
    if current_width < max_width {
        // we have some extra space left, let's add it to the last flexible part
        let extra = max_width - current_width;
        let mut last_part = split_parts.get_mut(last_flexible_index).unwrap();
        last_part.cols = Dimension::fixed(last_part.cols.as_usize() + extra);
        for part in (&mut split_parts[last_flexible_index + 1..]).iter_mut() {
            part.x += extra;
        }
    }*/
    split_parts
}

fn split_space_to_parts_horizontally(
    space_to_split: &PaneGeom,
    sizes: Vec<Option<SplitSize>>,
) -> Vec<PaneGeom> {
    let mut split_parts = Vec::new();
    let mut current_y_position = space_to_split.y;

    let mut parts_to_grow = Vec::new();

    for size in sizes {
        let rows = match size {
            Some(SplitSize::Percent(percent)) => Dimension::percent(percent as f64),
            Some(SplitSize::Fixed(size)) => Dimension::fixed(size as usize),
            None => {
                parts_to_grow.push(current_y_position);
                Dimension::fixed(1) // This is grown later on
            }
        };
        split_parts.push(PaneGeom {
            x: space_to_split.x,
            y: current_y_position,
            // FIXME: This is probably wrong
            cols: Dimension::percent(100.0),
            rows,
        });
        current_y_position += 1 + 1; // 1 for gap
    }

    split_parts
}

fn split_space(
    space_to_split: &PaneGeom,
    layout: &Layout,
) -> Vec<(Layout, PaneGeom)> {
    let mut pane_positions = Vec::new();
    let sizes: Vec<Option<SplitSize>> = layout.parts.iter().map(|part| part.split_size).collect();

    let split_parts = match layout.direction {
        Direction::Vertical => split_space_to_parts_vertically(space_to_split, sizes),
        Direction::Horizontal => split_space_to_parts_horizontally(space_to_split, sizes),
    };
    for (i, part) in layout.parts.iter().enumerate() {
        let part_position_and_size = split_parts.get(i).unwrap();
        if !part.parts.is_empty() {
            let mut part_positions = split_space(part_position_and_size, part);
            pane_positions.append(&mut part_positions);
        } else {
            pane_positions.push((part.clone(), *part_position_and_size));
        }
    }
    pane_positions
}
