from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

T = TypeVar("T", bound="UsageDay")


@_attrs_define
class UsageDay:
    """What an app's runs used on one UTC day (`date`, `YYYY-MM-DD`), each figure a decimal string; `wallMs` is information
    only.

        Attributes:
            date (str):
            runs (str): an unsigned 64-bit integer as a decimal string (D20)
            fuel (str): an unsigned 64-bit integer as a decimal string (D20)
            wall_ms (str): an unsigned 64-bit integer as a decimal string (D20)
            writes (str): an unsigned 64-bit integer as a decimal string (D20)
            bytes_written (str): an unsigned 64-bit integer as a decimal string (D20)
            events (str): an unsigned 64-bit integer as a decimal string (D20)
    """

    date: str
    runs: str
    fuel: str
    wall_ms: str
    writes: str
    bytes_written: str
    events: str
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        date = self.date

        runs = self.runs

        fuel = self.fuel

        wall_ms = self.wall_ms

        writes = self.writes

        bytes_written = self.bytes_written

        events = self.events

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "date": date,
                "runs": runs,
                "fuel": fuel,
                "wallMs": wall_ms,
                "writes": writes,
                "bytesWritten": bytes_written,
                "events": events,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        date = d.pop("date")

        runs = d.pop("runs")

        fuel = d.pop("fuel")

        wall_ms = d.pop("wallMs")

        writes = d.pop("writes")

        bytes_written = d.pop("bytesWritten")

        events = d.pop("events")

        usage_day = cls(
            date=date,
            runs=runs,
            fuel=fuel,
            wall_ms=wall_ms,
            writes=writes,
            bytes_written=bytes_written,
            events=events,
        )

        usage_day.additional_properties = d
        return usage_day

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
