from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

T = TypeVar("T", bound="OrgSettingsSet")


@_attrs_define
class OrgSettingsSet:
    """What `SetOrgSettings` answers: the settings after the change.

    Attributes:
        org (str):
        allow_guests (bool):
        allow_public_links (bool):
    """

    org: str
    allow_guests: bool
    allow_public_links: bool
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        org = self.org

        allow_guests = self.allow_guests

        allow_public_links = self.allow_public_links

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "org": org,
                "allowGuests": allow_guests,
                "allowPublicLinks": allow_public_links,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        org = d.pop("org")

        allow_guests = d.pop("allowGuests")

        allow_public_links = d.pop("allowPublicLinks")

        org_settings_set = cls(
            org=org,
            allow_guests=allow_guests,
            allow_public_links=allow_public_links,
        )

        org_settings_set.additional_properties = d
        return org_settings_set

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
