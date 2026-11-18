from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

T = TypeVar("T", bound="AppRunLogsResponse200LogsItem")


@_attrs_define
class AppRunLogsResponse200LogsItem:
    """
    Attributes:
        attempt (int):
        id (str):
        size (int):
        sha256 (str):
    """

    attempt: int
    id: str
    size: int
    sha256: str
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        attempt = self.attempt

        id = self.id

        size = self.size

        sha256 = self.sha256

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "attempt": attempt,
                "id": id,
                "size": size,
                "sha256": sha256,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        attempt = d.pop("attempt")

        id = d.pop("id")

        size = d.pop("size")

        sha256 = d.pop("sha256")

        app_run_logs_response_200_logs_item = cls(
            attempt=attempt,
            id=id,
            size=size,
            sha256=sha256,
        )

        app_run_logs_response_200_logs_item.additional_properties = d
        return app_run_logs_response_200_logs_item

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
