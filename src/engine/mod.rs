// SPDX-FileCopyrightText: 2026 Mel0nABC <mel0n@mel0n.dev>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Headless engine: geometry, zoom policy and session state.
//! Nothing here knows that GTK exists.

pub mod annotation;
pub mod geometry;
pub mod inertia;
pub mod page_range;
pub mod render;
pub mod search;
pub mod selection;
pub mod session;
pub mod zoom;
