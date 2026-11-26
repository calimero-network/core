from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from typing import cast

if TYPE_CHECKING:
    from ..models.version_deadline import VersionDeadline


T = TypeVar("T", bound="Deadlines")


@_attrs_define
class Deadlines:
    """Every release deadline after the change, oldest release first (`after` RFC 3339, `afterSecs` Unix seconds).

    Attributes:
        deadlines (list['VersionDeadline']):
    """

    deadlines: list["VersionDeadline"]
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.version_deadline import VersionDeadline

        deadlines = []
        for deadlines_item_data in self.deadlines:
            deadlines_item = deadlines_item_data.to_dict()
            deadlines.append(deadlines_item)

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "deadlines": deadlines,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.version_deadline import VersionDeadline

        d = dict(src_dict)
        deadlines = []
        _deadlines = d.pop("deadlines")
        for deadlines_item_data in _deadlines:
            deadlines_item = VersionDeadline.from_dict(deadlines_item_data)

            deadlines.append(deadlines_item)

        deadlines = cls(
            deadlines=deadlines,
        )

        deadlines.additional_properties = d
        return deadlines

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
