"""Shared helpers for cargo-4safer experiment scripts."""

import json
from pathlib import Path


REPO = Path(__file__).resolve().parents[1]
CONFIG_DIR = REPO / "scripts" / "config"
DEFAULT_EXPERIMENT = "budgeted"
DEFAULT_EXPERIMENT_CONFIG = CONFIG_DIR / f"{DEFAULT_EXPERIMENT}.json"
DEFAULT_EXPERIMENT_ROOT = REPO / "tmp" / "paper-experiments"


def repo_path(value, default=None):
    if value is None:
        return default
    path = Path(value)
    if path.is_absolute():
        return path
    return REPO / path


def load_json_object(path):
    with path.open("r", encoding="utf-8") as handle:
        payload = json.load(handle)
    if not isinstance(payload, dict):
        raise ValueError(f"{path} must contain a JSON object")
    return payload


def load_experiment_config(path):
    payload = load_json_object(path)
    experiment = payload.get("experiment")
    if not isinstance(experiment, str) or not experiment:
        raise ValueError(f"{path} must define a non-empty experiment name")
    for key in ("paths", "synthesize", "execute", "analyze"):
        value = payload.get(key, {})
        if value is not None and not isinstance(value, dict):
            raise ValueError(f"{path} {key} must be an object")
    return payload


def load_experiment_configs(config_dir=CONFIG_DIR):
    experiments = {}
    for path in sorted(config_dir.glob("*.json")):
        payload = load_experiment_config(path)
        experiment = payload["experiment"]
        if experiment in experiments:
            raise ValueError(f"duplicate experiment definition: {experiment}")
        experiments[experiment] = payload
    return experiments


def experiment_name(config):
    return config.get("experiment") or DEFAULT_EXPERIMENT


def path_section(config):
    paths = config.get("paths", {})
    if not isinstance(paths, dict):
        raise ValueError("paths must be an object")
    return paths


def configured_path(config, key, default=None):
    return repo_path(path_section(config).get(key), default)


def generation_results_path(config, experiment=None, default=None):
    paths = path_section(config)
    if paths.get("generation_results") is not None:
        return repo_path(paths["generation_results"], default)
    generation_out = repo_path(paths.get("generation_out"))
    if generation_out is not None:
        return generation_out / (experiment or experiment_name(config)) / "all_results.csv"
    return default


def execution_base_path(config, experiment=None, default=None):
    paths = path_section(config)
    if paths.get("execution_base") is not None:
        return repo_path(paths["execution_base"], default)
    execution_out = repo_path(paths.get("execution_out"))
    if execution_out is not None:
        return execution_out / (experiment or experiment_name(config))
    return default


def unsoundness_cases_path(config, experiment=None, default=None):
    paths = path_section(config)
    if paths.get("unsoundness_cases") is not None:
        return repo_path(paths["unsoundness_cases"], default)
    base = execution_base_path(config, experiment)
    if base is not None:
        return base / "unsoundness_cases.csv"
    return default


def analysis_out_path(config, experiment=None, default=None):
    paths = path_section(config)
    analysis_out = repo_path(paths.get("analysis_out"))
    if analysis_out is not None:
        return analysis_out / (experiment or experiment_name(config))
    return default
