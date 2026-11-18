from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from typing import cast

T = TypeVar("T", bound="ExportsExportResponse200")


@_attrs_define
class ExportsExportResponse200:
    """
    Attributes:
        app (str):
        id (str):
        size (int):
        sha256 (str):
        segments (list[str]):
        ephemeral (str):
        wrapped (str):
    """

    app: str
    id: str
    size: int
    sha256: str
    segments: list[str]
    ephemeral: str
    wrapped: str
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        app = self.app

        id = self.id

        size = self.size

        sha256 = self.sha256

        segments = self.segments

        ephemeral = self.ephemeral

        wrapped = self.wrapped

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "app": app,
                "id": id,
                "size": size,
                "sha256": sha256,
                "segments": segments,
                "ephemeral": ephemeral,
                "wrapped": wrapped,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        app = d.pop("app")

        id = d.pop("id")

        size = d.pop("size")

        sha256 = d.pop("sha256")

        segments = cast(list[str], d.pop("segments"))

        ephemeral = d.pop("ephemeral")

        wrapped = d.pop("wrapped")

        exports_export_response_200 = cls(
            app=app,
            id=id,
            size=size,
            sha256=sha256,
            segments=segments,
            ephemeral=ephemeral,
            wrapped=wrapped,
        )

        exports_export_response_200.additional_properties = d
        return exports_export_response_200

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
