"""Native production helpers for the finalized paired-yoke defense."""

from tools.gen_sprites import (
    IRON_DARK,
    IRON_LIGHT,
    SCRAP,
    SCRAP_DARK,
    SCRAP_LIGHT,
    s,
)

_BLACK = (10, 10, 13)


def _mix(
    start: tuple[int, int, int], end: tuple[int, int, int], amount: float
) -> tuple[int, int, int]:
    return tuple(int(left + (right - left) * amount) for left, right in zip(start, end))


def _level(charge: int, capacity: int) -> int:
    return max(0, min(capacity, charge))


def _cell_well(d, box, *, filled: bool) -> None:
    x0, y0, x1, y1 = box
    d.rounded_rectangle(
        [s(x0), s(y0), s(x1), s(y1)], radius=s(2), fill=(*IRON_DARK, 255)
    )
    color = SCRAP_LIGHT if filled else _mix(IRON_DARK, SCRAP_DARK, 0.36)
    d.rounded_rectangle(
        [s(x0 + 2), s(y0 + 2), s(x1 - 2), s(y1 - 2)], radius=s(1), fill=(*color, 255)
    )
    if filled:
        d.rectangle([s(x0 + 3), s(y0 + 2), s(x1 - 3), s(y0 + 3)], fill=(*SCRAP, 255))


def _four_cell_feed(d, charge: int, *, horizontal: bool) -> None:
    charge = _level(charge, 4)
    if horizontal:
        d.rounded_rectangle(
            [s(13), s(48), s(51), s(60)], radius=s(3), fill=(*IRON_DARK, 255)
        )
        for index in range(4):
            x0 = 16 + index * 8
            _cell_well(d, (x0, 51, x0 + 6, 57), filled=index < charge)
    else:
        d.rounded_rectangle(
            [s(49), s(20), s(61), s(57)], radius=s(3), fill=(*IRON_DARK, 255)
        )
        for index in range(4):
            y0 = 23 + (3 - index) * 8
            _cell_well(d, (52, y0, 58, y0 + 6), filled=index < charge)


def _flak_barrel_pair(d, *, xs: tuple[int, int], recoil: int, flash: bool) -> None:
    for x in xs:
        top = 3 + recoil
        bottom = 37 + recoil
        d.rounded_rectangle(
            [s(x - 3), s(top), s(x + 3), s(bottom)], radius=s(2), fill=(*IRON_DARK, 255)
        )
        d.rectangle(
            [s(x - 1), s(top + 4), s(x + 1), s(bottom - 3)], fill=(*IRON_LIGHT, 255)
        )
        d.ellipse([s(x - 3), s(top - 2), s(x + 3), s(top + 5)], fill=(*_BLACK, 255))
        if flash:
            d.polygon(
                [
                    (s(x), s(max(0, top - 7))),
                    (s(x - 5), s(max(0, top - 1))),
                    (s(x - 2), s(top + 1)),
                    (s(x), s(top + 5)),
                    (s(x + 2), s(top + 1)),
                    (s(x + 5), s(max(0, top - 1))),
                ],
                fill=(*SCRAP_LIGHT, 255),
            )


def _flak_cycle(phase: int) -> tuple[int, int, bool, bool]:
    return (
        (0, 0, False, False),
        (8, 1, True, False),
        (2, 8, False, True),
        (5, 3, False, False),
    )[phase]
