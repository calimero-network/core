from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from ..models.app_secrets_secrets_additional_property_source import (
    AppSecretsSecretsAdditionalPropertySource,
)
from ..types import UNSET, Unset
from typing import cast
from typing import Union

T = TypeVar("T", bound="AppSecretsSecretsAdditionalProperty")


@_attrs_define
class AppSecretsSecretsAdditionalProperty:
    """
    Attributes:
        host (str):
        paths (list[str]):
        binding (str):
        description (str):
        required (bool):
        source (AppSecretsSecretsAdditionalPropertySource):
        header (Union[Unset, str]):
        format_ (Union[Unset, str]):
        body_field (Union[Unset, str]):
        fingerprint (Union[Unset, str]):
        set_by (Union[Unset, str]):
        set_at (Union[Unset, int]):
    """

    host: str
    paths: list[str]
    binding: str
    description: str
    required: bool
    source: AppSecretsSecretsAdditionalPropertySource
    header: Union[Unset, str] = UNSET
    format_: Union[Unset, str] = UNSET
    body_field: Union[Unset, str] = UNSET
    fingerprint: Union[Unset, str] = UNSET
    set_by: Union[Unset, str] = UNSET
    set_at: Union[Unset, int] = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        host = self.host

        paths = self.paths

        binding = self.binding

        description = self.description

        required = self.required

        source = self.source.value

        header = self.header

        format_ = self.format_

        body_field = self.body_field

        fingerprint = self.fingerprint

        set_by = self.set_by

        set_at = self.set_at

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "host": host,
                "paths": paths,
                "binding": binding,
                "description": description,
                "required": required,
                "source": source,
            }
        )
        if header is not UNSET:
            field_dict["header"] = header
        if format_ is not UNSET:
            field_dict["format"] = format_
        if body_field is not UNSET:
            field_dict["bodyField"] = body_field
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
        host = d.pop("host")

        paths = cast(list[str], d.pop("paths"))

        binding = d.pop("binding")

        description = d.pop("description")

        required = d.pop("required")

        source = AppSecretsSecretsAdditionalPropertySource(d.pop("source"))

        header = d.pop("header", UNSET)

        format_ = d.pop("format", UNSET)

        body_field = d.pop("bodyField", UNSET)

        fingerprint = d.pop("fingerprint", UNSET)

        set_by = d.pop("setBy", UNSET)

        set_at = d.pop("setAt", UNSET)

        app_secrets_secrets_additional_property = cls(
            host=host,
            paths=paths,
            binding=binding,
            description=description,
            required=required,
            source=source,
            header=header,
            format_=format_,
            body_field=body_field,
            fingerprint=fingerprint,
            set_by=set_by,
            set_at=set_at,
        )

        app_secrets_secrets_additional_property.additional_properties = d
        return app_secrets_secrets_additional_property

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
