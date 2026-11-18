from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

T = TypeVar("T", bound="ConsentPageCreateResponse200")


@_attrs_define
class ConsentPageCreateResponse200:
    """
    Attributes:
        request_id (str):
        url (str):
        code (str):
        expires_at (int):
    """

    request_id: str
    url: str
    code: str
    expires_at: int
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        request_id = self.request_id

        url = self.url

        code = self.code

        expires_at = self.expires_at

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "requestId": request_id,
                "url": url,
                "code": code,
                "expiresAt": expires_at,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        request_id = d.pop("requestId")

        url = d.pop("url")

        code = d.pop("code")

        expires_at = d.pop("expiresAt")

        consent_page_create_response_200 = cls(
            request_id=request_id,
            url=url,
            code=code,
            expires_at=expires_at,
        )

        consent_page_create_response_200.additional_properties = d
        return consent_page_create_response_200

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
