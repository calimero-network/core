from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from typing import cast
from typing import cast, Union

if TYPE_CHECKING:
    from ..models.rollout_waves_item import RolloutWavesItem
    from ..models.rollout_paused_type_0 import RolloutPausedType0


T = TypeVar("T", bound="Rollout")


@_attrs_define
class Rollout:
    """How a deployment's offered release reaches its instances that apply updates on their own: wave `wave` of `waves`
    (1%, 10%, 50% and every instance, at least one, in an order each release draws afresh) is open; the next opens at
    `nextWaveAt` (`null` once the last is open or while paused), `waveHours` after the one before. `paused`: by a
    manager, or by `failures` once the failed upgrades and undos of the release pass `pausesAbovePercent` of the
    instances that tried it. `instances` run its package; `updated` run the release, `failed` failed to move to it,
    `undone` undid it.

        Attributes:
            app (str):
            package (str):
            version (str):
            offered_at (int):
            wave_hours (int):
            wave (int):
            waves (list['RolloutWavesItem']):
            next_wave_at (Union[None, int]):
            paused (Union['RolloutPausedType0', None]):
            instances (int):
            updated (int):
            failed (int):
            undone (int):
            pauses_above_percent (int):
    """

    app: str
    package: str
    version: str
    offered_at: int
    wave_hours: int
    wave: int
    waves: list["RolloutWavesItem"]
    next_wave_at: Union[None, int]
    paused: Union["RolloutPausedType0", None]
    instances: int
    updated: int
    failed: int
    undone: int
    pauses_above_percent: int
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.rollout_waves_item import RolloutWavesItem
        from ..models.rollout_paused_type_0 import RolloutPausedType0

        app = self.app

        package = self.package

        version = self.version

        offered_at = self.offered_at

        wave_hours = self.wave_hours

        wave = self.wave

        waves = []
        for waves_item_data in self.waves:
            waves_item = waves_item_data.to_dict()
            waves.append(waves_item)

        next_wave_at: Union[None, int]
        next_wave_at = self.next_wave_at

        paused: Union[None, dict[str, Any]]
        if isinstance(self.paused, RolloutPausedType0):
            paused = self.paused.to_dict()
        else:
            paused = self.paused

        instances = self.instances

        updated = self.updated

        failed = self.failed

        undone = self.undone

        pauses_above_percent = self.pauses_above_percent

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "app": app,
                "package": package,
                "version": version,
                "offeredAt": offered_at,
                "waveHours": wave_hours,
                "wave": wave,
                "waves": waves,
                "nextWaveAt": next_wave_at,
                "paused": paused,
                "instances": instances,
                "updated": updated,
                "failed": failed,
                "undone": undone,
                "pausesAbovePercent": pauses_above_percent,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.rollout_waves_item import RolloutWavesItem
        from ..models.rollout_paused_type_0 import RolloutPausedType0

        d = dict(src_dict)
        app = d.pop("app")

        package = d.pop("package")

        version = d.pop("version")

        offered_at = d.pop("offeredAt")

        wave_hours = d.pop("waveHours")

        wave = d.pop("wave")

        waves = []
        _waves = d.pop("waves")
        for waves_item_data in _waves:
            waves_item = RolloutWavesItem.from_dict(waves_item_data)

            waves.append(waves_item)

        def _parse_next_wave_at(data: object) -> Union[None, int]:
            if data is None:
                return data
            return cast(Union[None, int], data)

        next_wave_at = _parse_next_wave_at(d.pop("nextWaveAt"))

        def _parse_paused(data: object) -> Union["RolloutPausedType0", None]:
            if data is None:
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                paused_type_0 = RolloutPausedType0.from_dict(data)

                return paused_type_0
            except:  # noqa: E722
                pass
            return cast(Union["RolloutPausedType0", None], data)

        paused = _parse_paused(d.pop("paused"))

        instances = d.pop("instances")

        updated = d.pop("updated")

        failed = d.pop("failed")

        undone = d.pop("undone")

        pauses_above_percent = d.pop("pausesAbovePercent")

        rollout = cls(
            app=app,
            package=package,
            version=version,
            offered_at=offered_at,
            wave_hours=wave_hours,
            wave=wave,
            waves=waves,
            next_wave_at=next_wave_at,
            paused=paused,
            instances=instances,
            updated=updated,
            failed=failed,
            undone=undone,
            pauses_above_percent=pauses_above_percent,
        )

        rollout.additional_properties = d
        return rollout

    @property
    def additional_keys(self) -> list[str]:
        return list(self.additional_properties.keys())

    def __getitem__(self, key: str) -> Any:
        return self.additional_properties[key]

    def __setitem__(self, key: str, value: Any) -> None:
        self.additional_properties[key] = value

    def __delitem__(self, key: str) -> None:
        del self.additional_properties[key]

    def __contains__(self, key: str) -> bool:
        return key in self.additional_properties
