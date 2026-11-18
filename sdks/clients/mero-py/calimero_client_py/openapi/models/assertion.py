from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

T = TypeVar("T", bound="Assertion")


@_attrs_define
class Assertion:
    """The browser's answer to `navigator.credentials.get`, each field base64url.

    Attributes:
        credential_id (str):
        authenticator_data (str):
        client_data_json (str):
        signature (str):
    """

    credential_id: str
    authenticator_data: str
    client_data_json: str
    signature: str
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        credential_id = self.credential_id

        authenticator_data = self.authenticator_data

        client_data_json = self.client_data_json

        signature = self.signature

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "credentialId": credential_id,
                "authenticatorData": authenticator_data,
                "clientDataJson": client_data_json,
                "signature": signature,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        credential_id = d.pop("credentialId")

        authenticator_data = d.pop("authenticatorData")

        client_data_json = d.pop("clientDataJson")

        signature = d.pop("signature")

        assertion = cls(
            credential_id=credential_id,
            authenticator_data=authenticator_data,
            client_data_json=client_data_json,
            signature=signature,
        )

        assertion.additional_properties = d
        return assertion

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
