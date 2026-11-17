from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from ..models.app_secrets_response_200_secrets_additional_property_source import (
    AppSecretsResponse200SecretsAdditionalPropertySource,
)
from ..types import UNSET, Unset
from typing import cast
from typing import Union

T = TypeVar("T", bound="AppSecretsResponse200SecretsAdditionalProperty")


@_attrs_define
class AppSecretsResponse200SecretsAdditionalProperty:
    """
    Attributes:
        hosts (list[str]):
        description (str):
        required (bool):
        source (AppSecretsResponse200SecretsAdditionalPropertySource):
        fingerprint (Union[Unset, str]):
        set_by (Union[Unset, str]):
        set_at (Union[Unset, int]):
    """

    hosts: list[str]
    description: str
    required: bool
    source: AppSecretsResponse200SecretsAdditionalPropertySource
    fingerprint: Union[Unset, str] = UNSET
    set_by: Union[Unset, str] = UNSET
    set_at: Union[Unset, int] = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        hosts = self.hosts

        description = self.description

        required = self.required

        source = self.source.value

        fingerprint = self.fingerprint

        set_by = self.set_by

        set_at = self.set_at

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "hosts": hosts,
                "description": description,
                "required": required,
                "source": source,
            }
        )
        if fingerprint is not UNSET:
            field_dict["fingerprint"] = fingerprint
        if set_by is not UNSET:
            field_dict["setBy"] = set_by
        if set_at is not UNSET:
            field_dict["setAt"] = set_at

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        hosts = cast(list[str], d.pop("hosts"))

        description = d.pop("description")

        required = d.pop("required")

        source = AppSecretsResponse200SecretsAdditionalPropertySource(d.pop("source"))

        fingerprint = d.pop("fingerprint", UNSET)

        set_by = d.pop("setBy", UNSET)

        set_at = d.pop("setAt", UNSET)

        app_secrets_response_200_secrets_additional_property = cls(
            hosts=hosts,
            description=description,
            required=required,
            source=source,
            fingerprint=fingerprint,
            set_by=set_by,
            set_at=set_at,
        )

        app_secrets_response_200_secrets_additional_property.additional_properties = d
        return app_secrets_response_200_secrets_additional_property

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
