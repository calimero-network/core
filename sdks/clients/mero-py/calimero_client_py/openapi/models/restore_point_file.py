from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from typing import cast

if TYPE_CHECKING:
    from ..models.restore_point_file_source import RestorePointFileSource


T = TypeVar("T", bound="RestorePointFile")


@_attrs_define
class RestorePointFile:
    """
    Attributes:
        node (int):
        group (int):
        source (RestorePointFileSource):
    """

    node: int
    group: int
    source: "RestorePointFileSource"
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.restore_point_file_source import RestorePointFileSource

        node = self.node

        group = self.group

        source = self.source.to_dict()

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "node": node,
                "group": group,
                "source": source,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.restore_point_file_source import RestorePointFileSource

        d = dict(src_dict)
        node = d.pop("node")

        group = d.pop("group")

        source = RestorePointFileSource.from_dict(d.pop("source"))

        restore_point_file = cls(
            node=node,
            group=group,
            source=source,
        )

        restore_point_file.additional_properties = d
        return restore_point_file

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
