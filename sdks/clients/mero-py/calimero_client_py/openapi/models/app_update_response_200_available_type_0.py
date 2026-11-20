from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from ..models.app_update_response_200_available_type_0_source import (
    AppUpdateResponse200AvailableType0Source,
)

T = TypeVar("T", bound="AppUpdateResponse200AvailableType0")


@_attrs_define
class AppUpdateResponse200AvailableType0:
    """
    Attributes:
        package (str):
        version (str):
        source (AppUpdateResponse200AvailableType0Source):
    """

    package: str
    version: str
    source: AppUpdateResponse200AvailableType0Source
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        package = self.package

        version = self.version

        source = self.source.value

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "package": package,
                "version": version,
                "source": source,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        package = d.pop("package")

        version = d.pop("version")

        source = AppUpdateResponse200AvailableType0Source(d.pop("source"))

        app_update_response_200_available_type_0 = cls(
            package=package,
            version=version,
            source=source,
        )

        app_update_response_200_available_type_0.additional_properties = d
        return app_update_response_200_available_type_0

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
