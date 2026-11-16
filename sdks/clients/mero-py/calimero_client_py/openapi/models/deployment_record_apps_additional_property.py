from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

T = TypeVar("T", bound="DeploymentRecordAppsAdditionalProperty")


@_attrs_define
class DeploymentRecordAppsAdditionalProperty:
    """
    Attributes:
        package (str):
        version (str):
        added_by (str):
        added_at (int):
    """

    package: str
    version: str
    added_by: str
    added_at: int
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        package = self.package

        version = self.version

        added_by = self.added_by

        added_at = self.added_at

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "package": package,
                "version": version,
                "addedBy": added_by,
                "addedAt": added_at,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        package = d.pop("package")

        version = d.pop("version")

        added_by = d.pop("addedBy")

        added_at = d.pop("addedAt")

        deployment_record_apps_additional_property = cls(
            package=package,
            version=version,
            added_by=added_by,
            added_at=added_at,
        )

        deployment_record_apps_additional_property.additional_properties = d
        return deployment_record_apps_additional_property

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
