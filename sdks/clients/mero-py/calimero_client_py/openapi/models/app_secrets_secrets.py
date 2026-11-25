from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from typing import cast

if TYPE_CHECKING:
    from ..models.app_secrets_secrets_additional_property import (
        AppSecretsSecretsAdditionalProperty,
    )


T = TypeVar("T", bound="AppSecretsSecrets")


@_attrs_define
class AppSecretsSecrets:
    """ """

    additional_properties: dict[str, "AppSecretsSecretsAdditionalProperty"] = (
        _attrs_field(init=False, factory=dict)
    )

    def to_dict(self) -> dict[str, Any]:
        from ..models.app_secrets_secrets_additional_property import (
            AppSecretsSecretsAdditionalProperty,
        )

        field_dict: dict[str, Any] = {}
        for prop_name, prop in self.additional_properties.items():
            field_dict[prop_name] = prop.to_dict()

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.app_secrets_secrets_additional_property import (
            AppSecretsSecretsAdditionalProperty,
        )

        d = dict(src_dict)
        app_secrets_secrets = cls()

        additional_properties = {}
        for prop_name, prop_dict in d.items():
            additional_property = AppSecretsSecretsAdditionalProperty.from_dict(
                prop_dict
            )

            additional_properties[prop_name] = additional_property

        app_secrets_secrets.additional_properties = additional_properties
        return app_secrets_secrets

    @property
    def additional_keys(self) -> list[str]:
        return list(self.additional_properties.keys())

    def __getitem__(self, key: str) -> "AppSecretsSecretsAdditionalProperty":
        return self.additional_properties[key]

    def __setitem__(
        self, key: str, value: "AppSecretsSecretsAdditionalProperty"
    ) -> None:
        self.additional_properties[key] = value

    def __delitem__(self, key: str) -> None:
        del self.additional_properties[key]

    def __contains__(self, key: str) -> bool:
        return key in self.additional_properties
