from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from typing import cast, Union

T = TypeVar("T", bound="LinkView")


@_attrs_define
class LinkView:
    """A link as its list shows it: its key and terms, never a token.

    Attributes:
        link (str):
        label (Union[None, str]):
        expires_at (Union[None, int]):
        expired (bool):
        rate_per_minute (int):
        created_by (str):
        created_at (int):
    """

    link: str
    label: Union[None, str]
    expires_at: Union[None, int]
    expired: bool
    rate_per_minute: int
    created_by: str
    created_at: int
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        link = self.link

        label: Union[None, str]
        label = self.label

        expires_at: Union[None, int]
        expires_at = self.expires_at

        expired = self.expired

        rate_per_minute = self.rate_per_minute

        created_by = self.created_by

        created_at = self.created_at

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "link": link,
                "label": label,
                "expiresAt": expires_at,
                "expired": expired,
                "ratePerMinute": rate_per_minute,
                "createdBy": created_by,
                "createdAt": created_at,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        link = d.pop("link")

        def _parse_label(data: object) -> Union[None, str]:
            if data is None:
                return data
            return cast(Union[None, str], data)

        label = _parse_label(d.pop("label"))

        def _parse_expires_at(data: object) -> Union[None, int]:
            if data is None:
                return data
            return cast(Union[None, int], data)

        expires_at = _parse_expires_at(d.pop("expiresAt"))

        expired = d.pop("expired")

        rate_per_minute = d.pop("ratePerMinute")

        created_by = d.pop("createdBy")

        created_at = d.pop("createdAt")

        link_view = cls(
            link=link,
            label=label,
            expires_at=expires_at,
            expired=expired,
            rate_per_minute=rate_per_minute,
            created_by=created_by,
            created_at=created_at,
        )

        link_view.additional_properties = d
        return link_view

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
