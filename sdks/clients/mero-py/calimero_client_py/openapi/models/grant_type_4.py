from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from ..models.grant_type_4_kind import GrantType4Kind
from ..types import UNSET, Unset
from typing import Union

T = TypeVar("T", bound="GrantType4")


@_attrs_define
class GrantType4:
    """
    Attributes:
        kind (GrantType4Kind):
        link (str):
        rate_per_minute (int):
        created_by (str):
        created_at (int):
        label (Union[Unset, str]):
        expires_at (Union[Unset, int]):
    """

    kind: GrantType4Kind
    link: str
    rate_per_minute: int
    created_by: str
    created_at: int
    label: Union[Unset, str] = UNSET
    expires_at: Union[Unset, int] = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        kind = self.kind.value

        link = self.link

        rate_per_minute = self.rate_per_minute

        created_by = self.created_by

        created_at = self.created_at

        label = self.label

        expires_at = self.expires_at

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "kind": kind,
                "link": link,
                "ratePerMinute": rate_per_minute,
                "createdBy": created_by,
                "createdAt": created_at,
            }
        )
        if label is not UNSET:
            field_dict["label"] = label
        if expires_at is not UNSET:
            field_dict["expiresAt"] = expires_at

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        kind = GrantType4Kind(d.pop("kind"))

        link = d.pop("link")

        rate_per_minute = d.pop("ratePerMinute")

        created_by = d.pop("createdBy")

        created_at = d.pop("createdAt")

        label = d.pop("label", UNSET)

        expires_at = d.pop("expiresAt", UNSET)

        grant_type_4 = cls(
            kind=kind,
            link=link,
            rate_per_minute=rate_per_minute,
            created_by=created_by,
            created_at=created_at,
            label=label,
            expires_at=expires_at,
        )

        grant_type_4.additional_properties = d
        return grant_type_4

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
