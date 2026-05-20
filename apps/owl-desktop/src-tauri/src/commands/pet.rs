//! Tauri commands for the Owl chibi pet — thin wrappers over [`PetState`].
//!
//! Frontend invokes:
//! - `get_pet()`              → current snapshot for first render
//! - `feed_pet(amount?)`      → +hunger, "nom nom" bubble
//! - `play_pet()`             → +happiness / -energy
//! - `pet_pet()`              → +happiness (gentle)
//! - `rename_pet(name)`       → set display name
//!
//! Every mutating command updates `PetState` and broadcasts a `pet_event`
//! Tauri channel message; the widget refreshes automatically.

use tauri::State;

use owl_protocol::pet::{Pet, PetEvent};

use crate::state::AppState;

#[tauri::command]
pub async fn get_pet(state: State<'_, AppState>) -> Result<Pet, String> {
    Ok(state.pet.snapshot().await)
}

#[tauri::command]
pub async fn feed_pet(amount: Option<u32>, state: State<'_, AppState>) -> Result<Pet, String> {
    state.pet.apply(PetEvent::Fed { amount: amount.unwrap_or(30) }).await;
    Ok(state.pet.snapshot().await)
}

#[tauri::command]
pub async fn play_pet(state: State<'_, AppState>) -> Result<Pet, String> {
    state.pet.apply(PetEvent::Played).await;
    Ok(state.pet.snapshot().await)
}

#[tauri::command]
pub async fn pet_pet(state: State<'_, AppState>) -> Result<Pet, String> {
    state.pet.apply(PetEvent::Petted).await;
    Ok(state.pet.snapshot().await)
}

#[tauri::command]
pub async fn rename_pet(name: String, state: State<'_, AppState>) -> Result<Pet, String> {
    let trimmed = name.trim();
    if trimmed.is_empty() || trimmed.len() > 32 {
        return Err("pet name must be 1–32 chars".into());
    }
    state.pet.rename(trimmed.to_string()).await;
    Ok(state.pet.snapshot().await)
}

/// Dev cheat — set pet level + XP directly.  Useful for skipping the
/// egg/chick stages while iterating on UI or sprite frames.
#[tauri::command]
pub async fn set_pet_level(
    level: u32,
    xp:    Option<u32>,
    state: State<'_, AppState>,
) -> Result<Pet, String> {
    state.pet.set_level(level, xp.unwrap_or(0)).await;
    Ok(state.pet.snapshot().await)
}
