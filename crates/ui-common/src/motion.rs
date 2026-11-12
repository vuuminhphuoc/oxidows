//! Motion and animations for Windows 11 parity: hover fades, sliding selection indicator,
//! and navigation pane collapse/expand.

use std::sync::Mutex;
use std::time::{Duration, Instant};

const CACHE_TTL: Duration = Duration::from_secs(3);

#[derive(Clone, Copy)]
struct CacheState {
    value: bool,
    last_checked: Instant,
}

static SYSTEM_ANIM_CACHE: Mutex<Option<CacheState>> = Mutex::new(None);

/// Checks if a cache entry recorded at `cached_at` is still within `ttl` of `now`.
pub fn is_cache_valid(cached_at: Instant, now: Instant, ttl: Duration) -> bool {
    now.checked_duration_since(cached_at).is_some_and(|d| d < ttl)
}

#[cfg(windows)]
fn query_client_area_animation() -> bool {
    use std::ffi::c_void;
    use windows_sys::Win32::UI::WindowsAndMessaging::{SPI_GETCLIENTAREAANIMATION, SystemParametersInfoW};

    let mut anim: i32 = 0;
    // SAFETY: SystemParametersInfoW receives a valid BOOL out-pointer with pvParam.
    let ok = unsafe { SystemParametersInfoW(SPI_GETCLIENTAREAANIMATION, 0, (&raw mut anim).cast::<c_void>(), 0) };
    ok != 0 && anim != 0
}

fn check_system_animations(now: Instant) -> bool {
    #[cfg(windows)]
    {
        let mut guard = SYSTEM_ANIM_CACHE.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(state) = *guard
            && is_cache_valid(state.last_checked, now, CACHE_TTL)
        {
            return state.value;
        }
        let val = query_client_area_animation();
        *guard = Some(CacheState { value: val, last_checked: now });
        val
    }
    #[cfg(not(windows))]
    {
        let _ = now;
        true
    }
}

/// Whether UI animations are enabled on this system.
/// False if env `OXI_NO_ANIM` is set, or on Windows if client area animation is turned off.
pub fn animations_enabled(_ctx: &egui::Context) -> bool {
    if std::env::var_os("OXI_NO_ANIM").is_some() {
        return false;
    }
    check_system_animations(Instant::now())
}

/// Fades a boolean state in (0..=1) over 0.10 s using egui's animation manager.
/// Returns the target immediately (1.0 or 0.0) when animations are disabled.
pub fn fade(ctx: &egui::Context, id: egui::Id, on: bool) -> f32 {
    if !animations_enabled(ctx) { if on { 1.0 } else { 0.0 } } else { ctx.animate_bool_with_time(id, on, 0.10) }
}

/// Linearly animates a float value to `target` over `duration` seconds.
/// Returns `target` immediately when animations are disabled.
pub fn animate_value(ctx: &egui::Context, id: egui::Id, target: f32, duration: f32) -> f32 {
    if !animations_enabled(ctx) { target } else { ctx.animate_value_with_time(id, target, duration) }
}

/// Standard cubic ease-out curve ($f(t) = 1 - (1 - t)^3$).
pub fn ease_out(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    1.0 - (1.0 - t).powi(3)
}

/// Linear interpolation between `a` and `b`.
pub fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

/// Interpolates between `none` and `hover` colors by `t` (0.0 to 1.0).
pub fn lerp_color(none: egui::Color32, hover: egui::Color32, t: f32) -> egui::Color32 {
    let t = t.clamp(0.0, 1.0);
    if t <= 0.0 {
        none
    } else if t >= 1.0 {
        hover
    } else if none == egui::Color32::TRANSPARENT || none.a() == 0 {
        egui::Color32::from_rgba_unmultiplied(hover.r(), hover.g(), hover.b(), (hover.a() as f32 * t).round() as u8)
    } else {
        let l = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * t).round() as u8;
        egui::Color32::from_rgba_unmultiplied(
            l(none.r(), hover.r()),
            l(none.g(), hover.g()),
            l(none.b(), hover.b()),
            l(none.a(), hover.a()),
        )
    }
}

#[derive(Clone, Copy)]
struct IndicatorState {
    from_y: f32,
    from_h: f32,
    target_y: f32,
    target_h: f32,
    start_time: f64,
}

/// Slides the navigation indicator to (`target_y`, `target_h`) over 0.15 s with ease-out.
/// Only requests repaints while the animation is actively running.
pub fn slide_indicator(ctx: &egui::Context, id: egui::Id, target_y: f32, target_h: f32) -> (f32, f32) {
    if !animations_enabled(ctx) {
        return (target_y, target_h);
    }
    let now = ctx.input(|i| i.time);
    let mut state = ctx.data_mut(|d| {
        *d.get_temp_mut_or_insert_with(id, || IndicatorState {
            from_y: target_y,
            from_h: target_h,
            target_y,
            target_h,
            start_time: now,
        })
    });

    if (state.target_y - target_y).abs() > 0.5 || (state.target_h - target_h).abs() > 0.5 {
        let elapsed = (now - state.start_time).max(0.0) as f32;
        let t = (elapsed / 0.15).min(1.0);
        let curr_y = lerp(state.from_y, state.target_y, ease_out(t));
        let curr_h = lerp(state.from_h, state.target_h, ease_out(t));
        state.from_y = curr_y;
        state.from_h = curr_h;
        state.target_y = target_y;
        state.target_h = target_h;
        state.start_time = now;
        ctx.data_mut(|d| d.insert_temp(id, state));
    }

    let elapsed = (now - state.start_time).max(0.0) as f32;
    if elapsed < 0.15 {
        ctx.request_repaint();
        let t = (elapsed / 0.15).min(1.0);
        (lerp(state.from_y, state.target_y, ease_out(t)), lerp(state.from_h, state.target_h, ease_out(t)))
    } else {
        (state.target_y, state.target_h)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ease_out_curve() {
        assert_eq!(ease_out(0.0), 0.0);
        assert_eq!(ease_out(1.0), 1.0);
        assert_eq!(ease_out(-0.5), 0.0);
        assert_eq!(ease_out(1.5), 1.0);
        // Ease-out starts faster: at t = 0.5, value is 1 - 0.5^3 = 0.875
        assert!((ease_out(0.5) - 0.875).abs() < 1e-6);
        assert!(ease_out(0.25) < ease_out(0.75));
    }

    #[test]
    fn lerp_scalars_and_colors() {
        assert_eq!(lerp(10.0, 20.0, 0.0), 10.0);
        assert_eq!(lerp(10.0, 20.0, 1.0), 20.0);
        assert_eq!(lerp(10.0, 20.0, 0.5), 15.0);

        let none = egui::Color32::TRANSPARENT;
        let hover = egui::Color32::from_rgb(100, 150, 200);
        assert_eq!(lerp_color(none, hover, 0.0), none);
        assert_eq!(lerp_color(none, hover, 1.0), hover);

        let mid = lerp_color(none, hover, 0.5);
        assert_eq!(mid.a(), 128);
        let unmult = mid.to_srgba_unmultiplied();
        assert!((unmult[0] as i16 - 100).abs() <= 1);
        assert!((unmult[1] as i16 - 150).abs() <= 1);
        assert!((unmult[2] as i16 - 200).abs() <= 1);
        assert_eq!(unmult[3], 128);
    }

    #[test]
    fn cache_expiry() {
        let t0 = Instant::now();
        let ttl = Duration::from_secs(3);
        assert!(is_cache_valid(t0, t0 + Duration::from_secs(1), ttl));
        assert!(is_cache_valid(t0, t0 + Duration::from_secs(2), ttl));
        assert!(!is_cache_valid(t0, t0 + Duration::from_secs(3), ttl));
        assert!(!is_cache_valid(t0, t0 + Duration::from_secs(5), ttl));
        assert!(!is_cache_valid(t0 + Duration::from_secs(1), t0, ttl));
    }
}
