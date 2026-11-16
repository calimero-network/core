from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

T = TypeVar("T", bound="PackageOrigin")


@_attrs_define
class PackageOrigin:
    """A custom frontend origin of a package: the rp-id its passkeys use, who registered it and when (RFC 3339), and
    whether the cluster serves the package's frontend there.

        Attributes:
            package (str):
            origin (str):
            rp_id (str):
            added_by (str):
            added_at (str):
            served (bool):
    """

    package: str
    origin: str
    rp_id: str
    added_by: str
    added_at: str
    served: bool
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        package = self.package

        origin = self.origin

        rp_id = self.rp_id

        added_by = self.added_by

        added_at = self.added_at

        served = self.served

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "package": package,
                "origin": origin,
                "rpId": rp_id,
                "addedBy": added_by,
                "addedAt": added_at,
                "served": served,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        package = d.pop("package")

        origin = d.pop("origin")

        rp_id = d.pop("rpId")

        added_by = d.pop("addedBy")

        added_at = d.pop("addedAt")

        served = d.pop("served")

        package_origin = cls(
            package=package,
            origin=origin,
            rp_id=rp_id,
            added_by=added_by,
            added_at=added_at,
            served=served,
        )

        package_origin.additional_properties = d
        return package_origin

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
