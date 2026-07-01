#!/usr/bin/env python3
"""Draw a single-column sequence-length distribution PDF.

The plotted length is target + mutator depth:

    sequence length = 1 + mutator_count
"""

import argparse
import csv
import json
from collections import Counter
from pathlib import Path

import matplotlib

matplotlib.use("pdf")
import matplotlib.pyplot as plt

from experiment_common import DEFAULT_EXPERIMENT, DEFAULT_EXPERIMENT_ROOT, REPO

DEFAULT_INPUT = DEFAULT_EXPERIMENT_ROOT / "runs" / DEFAULT_EXPERIMENT / "tests"
DEFAULT_CSV = REPO / "misc/trophy_crates.csv"
DEFAULT_OUT = DEFAULT_EXPERIMENT_ROOT / "runs" / "sequence_length_distribution.pdf"

COLORS = {
    1: "#4E79A7",
    2: "#F28E2B",
    3: "#59A14F",
    4: "#E15759",
    5: "#B07AA1",
}


def main():
    args = parse_args()
    rows = load_rows(args.input_dir, args.csv)
    if not rows:
        raise SystemExit(f"no result.json files found under {args.input_dir}")

    total_counts = Counter()
    for row in rows:
        total_counts.update(row["length_counts"])
    rows.append(
        {
            "label": "Total",
            "length_counts": dict(total_counts),
            "total": sum(total_counts.values()),
        }
    )

    args.out.parent.mkdir(parents=True, exist_ok=True)
    draw_pdf(rows, args.out)
    print(args.out)


def parse_args():
    parser = argparse.ArgumentParser(
        description="Draw sequence length distribution as a single-column PDF figure."
    )
    parser.add_argument(
        "--input-dir",
        type=Path,
        default=DEFAULT_INPUT,
        help=f"backend result directory containing <crate>/result.json files (default: {DEFAULT_INPUT})",
    )
    parser.add_argument(
        "--csv",
        type=Path,
        default=DEFAULT_CSV,
        help=f"crate CSV used to preserve row order (default: {DEFAULT_CSV})",
    )
    parser.add_argument(
        "--out",
        type=Path,
        default=DEFAULT_OUT,
        help=f"output PDF path (default: {DEFAULT_OUT})",
    )
    return parser.parse_args()


def load_rows(input_dir, csv_path):
    by_key = {}
    for path in sorted(input_dir.glob("*-*/result.json")):
        result = json.loads(path.read_text(encoding="utf-8"))
        key = f"{result['crate']}-{result['version']}"
        counts = {
            int(mutator_count) + 1: int(count)
            for mutator_count, count in result.get("mutator_count_sequence_counts", {}).items()
        }
        by_key[key] = {
            "label": result["crate"],
            "length_counts": counts,
            "total": int(result.get("sequence_count") or sum(counts.values())),
        }

    ordered = []
    seen = set()
    if csv_path.exists():
        with csv_path.open(encoding="utf-8-sig", newline="") as handle:
            for row in csv.DictReader(handle):
                name = (row.get("crate") or "").strip()
                version = (row.get("version") or "").strip()
                key = f"{name}-{version}"
                if key in by_key:
                    ordered.append(by_key[key])
                    seen.add(key)

    ordered.extend(row for key, row in sorted(by_key.items()) if key not in seen)
    return ordered


def draw_pdf(rows, out_path):
    plt.rcParams.update(
        {
            "pdf.fonttype": 42,
            "ps.fonttype": 42,
            "font.family": "DejaVu Sans",
            "font.size": 6.5,
            "axes.linewidth": 0.4,
            "xtick.major.width": 0.4,
            "ytick.major.width": 0.0,
        }
    )

    labels = [row["label"] for row in rows]
    y_positions = list(range(len(rows)))
    fig_height = 0.205 * len(rows) + 0.52
    fig, ax = plt.subplots(figsize=(3.35, fig_height), constrained_layout=False)

    ax.set_xlim(0, 1.16)
    ax.set_ylim(-0.9, len(rows) - 0.45)
    ax.invert_yaxis()
    ax.set_yticks(y_positions)
    ax.set_yticklabels(labels, fontsize=6.2)
    ax.set_xticks([])
    ax.tick_params(axis="x", length=0)
    ax.tick_params(axis="y", length=0, pad=2.0)
    ax.grid(axis="x", color="#d0d7de", linewidth=0.35, linestyle="-", zorder=0)

    for spine in ["left", "right", "bottom", "top"]:
        ax.spines[spine].set_visible(False)

    add_header(ax)

    bar_height = 0.68
    for y, row in zip(y_positions, rows):
        if row["label"] == "Total":
            ax.axhline(y - 0.55, color="#8c959f", linewidth=0.55, clip_on=False)
        draw_row(ax, y, row, bar_height)

    fig.subplots_adjust(left=0.285, right=0.965, top=0.9, bottom=0.028)
    fig.savefig(out_path, format="pdf", bbox_inches="tight", pad_inches=0.015)
    plt.close(fig)


def add_header(ax):
    legend_y = -0.68
    x = 0.0
    ax.text(
        x,
        legend_y,
        "length",
        ha="left",
        va="center",
        fontsize=5.8,
        color="#4b5563",
        clip_on=False,
    )
    x += 0.17
    for length in sorted(COLORS):
        ax.scatter(
            [x],
            [legend_y],
            marker="s",
            s=18,
            color=COLORS[length],
            edgecolors="none",
            clip_on=False,
            zorder=4,
        )
        ax.text(
            x + 0.025,
            legend_y,
            str(length),
            ha="left",
            va="center",
            fontsize=5.8,
            color="#4b5563",
            clip_on=False,
        )
        x += 0.115
    ax.text(
        1.11,
        legend_y,
        "#suite",
        ha="right",
        va="center",
        fontsize=5.8,
        color="#4b5563",
        clip_on=False,
    )


def draw_row(ax, y, row, bar_height):
    total = row["total"]
    left = 0.0
    for length in sorted(COLORS):
        count = int(row["length_counts"].get(length, 0))
        if count == 0 or total == 0:
            continue
        width = count / total
        color = COLORS[length]
        ax.barh(
            y,
            width,
            left=left,
            height=bar_height,
            color=color,
            edgecolor="white",
            linewidth=0.25,
            zorder=2,
        )
        label_segment(ax, left, width, y, count, color, bar_height)
        left += width
    ax.text(1.11, y, f"{total}", ha="right", va="center", fontsize=6.0, color="#1f2328")


def label_segment(ax, left, width, y, count, color, bar_height):
    x = left + width / 2
    if width >= 0.06:
        text_color = "white" if is_dark(color) else "#1f2328"
        ax.text(
            x,
            y,
            str(count),
            ha="center",
            va="center",
            fontsize=5.25,
            fontweight="bold",
            color=text_color,
            zorder=3,
        )
        return

    ax.text(
        x,
        y,
        str(count),
        ha="center",
        va="center",
        fontsize=4.0,
        color="#1f2328",
        zorder=3,
        bbox={
            "boxstyle": "round,pad=0.08",
            "facecolor": "white",
            "edgecolor": "none",
            "alpha": 0.82,
        },
    )


def is_dark(color):
    red = int(color[1:3], 16)
    green = int(color[3:5], 16)
    blue = int(color[5:7], 16)
    luminance = 0.2126 * red + 0.7152 * green + 0.0722 * blue
    return luminance < 145


if __name__ == "__main__":
    main()
