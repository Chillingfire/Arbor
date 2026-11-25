use std::fs;

use arbor_config::{
    Config, ProbeCache, RoleConfig, Roles, max_temperature, probe_cache_path, save_api_key,
    save_changed_roles,
};
use tempfile::tempdir;

fn role(model: &str, temperature: f64) -> RoleConfig {
    RoleConfig {
        model: model.to_owned(),
        temperature,
    }
}

#[test]
fn anthropic_models_take_temperatures_up_to_one_and_others_up_to_two() {
    assert_eq!(max_temperature("anthropic/claude-haiku-5.5"), 1.0);
    assert_eq!(max_temperature("openai/gpt-6"), 2.0);
    assert!(
        role("anthropic/claude-haiku-5.5", 1.0)
            .check_temperature("Writer")
            .is_ok()
    );
    assert!(
        role("openai/gpt-6", 1.5)
            .check_temperature("Writer")
            .is_ok()
    );
    assert!(
        role("openai/gpt-6", 0.0)
            .check_temperature("Writer")
            .is_ok()
    );
}

#[test]
fn a_temperature_outside_the_range_names_the_role_and_the_limit() {
    let problem = role("anthropic/claude-haiku-5.5", 1.5)
        .check_temperature("Writer")
        .unwrap_err();
    assert_eq!(
        problem.to_string(),
        "the Writer temperature 1.5 is outside 0–1 for anthropic/claude-haiku-5.5"
    );
    assert!(
        role("openai/gpt-6", -0.1)
            .check_temperature("Writer")
            .is_err()
    );
    assert!(
        role("openai/gpt-6", 2.1)
            .check_temperature("Writer")
            .is_err()
    );
    assert!(
        role("openai/gpt-6", f64::NAN)
            .check_temperature("Writer")
            .is_err()
    );
}

#[test]
fn the_recommended_roles_pass_the_range_check() {
    assert!(Roles::recommended().temperature_problems().is_empty());
    let mut roles = Roles::recommended();
    roles.scaffolder.temperature = 3.0;
    let problems = roles.temperature_problems();
    assert_eq!(problems.len(), 1);
    assert_eq!(problems[0].role, "Scaffolder");
}

#[test]
fn the_probe_cache_sits_next_to_the_user_config() {
    let dir = tempdir().unwrap();
    let user_config = dir.path().join("arbor").join("config.toml");
    assert_eq!(
        probe_cache_path(&user_config),
        dir.path().join("arbor").join("probes.toml")
    );
}

#[test]
fn a_recorded_probe_is_kept_per_model_and_temperature() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("arbor").join("probes.toml");
    let mut cache = ProbeCache::load(&path);
    assert_eq!(cache, ProbeCache::default());

    cache.record(&role("x/writer", 0.0));
    cache.record(&role("x/writer", 0.0));
    cache.save(&path).unwrap();

    let reloaded = ProbeCache::load(&path);
    assert_eq!(reloaded, cache);
    assert!(reloaded.contains(&role("x/writer", 0.0)));
    assert!(!reloaded.contains(&role("x/writer", 0.2)));
    assert!(!reloaded.contains(&role("x/planner", 0.0)));
}

#[test]
fn a_malformed_probe_cache_counts_as_empty() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("probes.toml");
    fs::write(&path, "not [toml").unwrap();
    assert_eq!(ProbeCache::load(&path), ProbeCache::default());
}

#[test]
fn unprobed_lists_each_uncached_model_and_temperature_once() {
    let mut roles = Roles::recommended();
    roles.scaffolder = roles.planner.clone();
    let mut cache = ProbeCache::default();
    assert_eq!(
        cache.unprobed(&roles),
        vec![roles.planner.clone(), roles.writer.clone()]
    );
    cache.record(&roles.writer);
    assert_eq!(cache.unprobed(&roles), vec![roles.planner.clone()]);
    cache.record(&roles.planner);
    assert!(cache.unprobed(&roles).is_empty());
}

#[test]
fn saved_roles_load_back_and_keep_the_rest_of_the_user_config() {
    let dir = tempdir().unwrap();
    let user_config = dir.path().join("arbor").join("config.toml");
    save_api_key(&user_config, "sk-or-kept").unwrap();
    fs::write(
        &user_config,
        fs::read_to_string(&user_config).unwrap() + "[roles.writer]\nmodel = \"x/old\"\n",
    )
    .unwrap();
    let previous = Config::load(Some(&user_config), dir.path()).unwrap().roles;
    let mut roles = previous.clone();
    roles.writer = role("x/writer", 0.2);

    save_changed_roles(&user_config, &roles, &previous).unwrap();

    let saved_toml = fs::read_to_string(&user_config).unwrap();
    assert!(saved_toml.contains("sk-or-kept"));
    let config = Config::load(Some(&user_config), dir.path()).unwrap();
    assert_eq!(config.roles, roles);
}

#[test]
fn saving_roles_writes_only_the_roles_that_changed() {
    let dir = tempdir().unwrap();
    let user_config = dir.path().join("config.toml");
    let previous = Roles::recommended();
    let mut roles = previous.clone();
    roles.writer.temperature = 0.2;

    save_changed_roles(&user_config, &roles, &previous).unwrap();

    let saved_toml = fs::read_to_string(&user_config).unwrap();
    assert!(saved_toml.contains("writer"));
    assert!(!saved_toml.contains("planner"));
    assert!(!saved_toml.contains("scaffolder"));
}

#[test]
fn a_role_set_back_to_its_recommendation_loses_its_entry() {
    let dir = tempdir().unwrap();
    let user_config = dir.path().join("config.toml");
    fs::write(
        &user_config,
        "api_key = \"sk-or-kept\"\n\n[roles.planner]\nmodel = \"x/planner\"\ntemperature = 0.9\n",
    )
    .unwrap();
    let previous = Config::load(Some(&user_config), dir.path()).unwrap().roles;

    save_changed_roles(&user_config, &Roles::recommended(), &previous).unwrap();

    let saved_toml = fs::read_to_string(&user_config).unwrap();
    assert!(!saved_toml.contains("roles"));
    assert!(saved_toml.contains("sk-or-kept"));
    let config = Config::load(Some(&user_config), dir.path()).unwrap();
    assert_eq!(config.roles, Roles::recommended());
}

#[test]
fn saving_roles_never_overwrites_an_unreadable_config() {
    let dir = tempdir().unwrap();
    let user_config = dir.path().join("config.toml");
    fs::write(&user_config, "api_key = ").unwrap();
    let mut roles = Roles::recommended();
    roles.writer.temperature = 0.2;
    assert!(save_changed_roles(&user_config, &roles, &Roles::recommended()).is_err());
    assert_eq!(fs::read_to_string(&user_config).unwrap(), "api_key = ");
}
