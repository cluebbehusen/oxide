"""Canonical full-hull construction frames for every Oxide building."""

from __future__ import annotations

from pathlib import Path

from PIL import Image, ImageDraw

from tools import gen_sprites as gen

Registry = dict[str, Image.Image]

BUILDING_STEMS = (
    "foundry",
    "turret",
    "fabricator",
    "flak_turret",
    "bastion",
    "array",
    "reclaimer",
    "repair_bay",
    "extractor",
    "airworks",
    "crucible",
    "barricade",
    "scuttle_charge",
)

DEFENSE_MOUNTS = {
    "turret": "turret_barrel",
    "flak_turret": "flak_mount",
    "bastion": "bastion_mount",
}

HULL_OPACITY = (96, 160, 224)


def complete_hull(registry: Registry, stem: str, faction: str) -> Image.Image:
    """Returns the recognizable final hull, including a defense's mount."""
    hull = registry[f"{stem}_{faction}"].convert("RGBA").copy()
    if mount_stem := DEFENSE_MOUNTS.get(stem):
        hull.alpha_composite(registry[f"{mount_stem}_{faction}"].convert("RGBA"))
    return hull


def _scaled(value: float, scale: float) -> int:
    return round(value * scale)


def dimmed_hull(source: Image.Image, stage: int) -> Image.Image:
    """Keeps every hull pixel while raising its opacity with progress."""
    opacity = HULL_OPACITY[stage]
    hull = source.copy()
    hull.putalpha(
        source.getchannel("A").point(
            lambda value: 0 if value == 0 else max(1, value * opacity // 255)
        )
    )
    return hull


def construction_frame(
    registry: Registry,
    stem: str,
    faction: str,
    stage: int,
    phase: int,
) -> Image.Image:
    """Builds one full-hull scaffold-cage frame at native game scale."""
    source = complete_hull(registry, stem, faction)
    width, height = source.size
    scale = width / 64
    frame = Image.new("RGBA", source.size, (0, 0, 0, 0))
    frame.alpha_composite(dimmed_hull(source, stage))

    draw = ImageDraw.Draw(frame)
    post_width = max(2, _scaled(2, scale))
    rail_width = max(1, _scaled(1, scale))
    left_post = _scaled(6, scale)
    right_post = width - _scaled(6, scale)
    top = _scaled(8, scale)
    bottom = height - _scaled(6, scale)
    crossbars = (_scaled(9, scale), _scaled(32, scale), height - _scaled(7, scale))
    scaffold = (*gen.IRON_LIGHT, 232)
    scaffold_shadow = (*gen.IRON_DARK, 244)

    for x in (left_post, right_post):
        draw.line((x, top, x, bottom), fill=scaffold_shadow, width=post_width + 2)
        draw.line((x, top, x, bottom), fill=scaffold, width=post_width)
    for y in crossbars:
        draw.line(
            (left_post, y, right_post, y),
            fill=scaffold_shadow,
            width=rail_width + 2,
        )
        draw.line((left_post, y, right_post, y), fill=scaffold, width=rail_width)

    contact_x = right_post - _scaled(8, scale)
    contact_y = crossbars[1] + _scaled(8, scale)
    contact_radius = max(1, _scaled(2, scale))
    contact = gen.SCRAP_LIGHT if phase else gen.SCRAP_DARK
    draw.rectangle(
        (
            contact_x - contact_radius,
            contact_y - contact_radius,
            contact_x + contact_radius,
            contact_y + contact_radius,
        ),
        fill=(*contact, 255),
    )
    if phase:
        spark = max(2, _scaled(3, scale))
        draw.line(
            (contact_x - spark, contact_y, contact_x + spark, contact_y),
            fill=(*gen.SCRAP_LIGHT, 255),
            width=max(1, _scaled(1, scale)),
        )
        draw.line(
            (contact_x, contact_y - spark, contact_x, contact_y + spark),
            fill=(*gen.FACTIONS[faction]["light"], 255),
            width=max(1, _scaled(1, scale)),
        )
    return frame


def install_finalized_construction(registry: Registry, out: Path) -> None:
    """Installs the shared full-hull construction treatment."""
    out.mkdir(parents=True, exist_ok=True)
    for faction in gen.FACTIONS:
        for stem in BUILDING_STEMS:
            for stage in range(3):
                for phase in range(2):
                    name = f"{stem}_{faction}_site{stage}_{phase}"
                    frame = construction_frame(registry, stem, faction, stage, phase)
                    frame.save(out / f"{name}.png")
                    registry[name] = frame
