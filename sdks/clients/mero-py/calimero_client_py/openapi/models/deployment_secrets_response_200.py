from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from typing import cast

if TYPE_CHECKING:
    from ..models.deployment_secrets_response_200_secrets import (
        DeploymentSecretsResponse200Secrets,
    )


T = TypeVar("T", bound="DeploymentSecretsResponse200")


@_attrs_define
class DeploymentSecretsResponse200:
    """
    Attributes:
        deployment (str):
        secrets (DeploymentSecretsResponse200Secrets):
    """

    deployment: str
    secrets: "DeploymentSecretsResponse200Secrets"
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.deployment_secrets_response_200_secrets import (
            DeploymentSecretsResponse200Secrets,
        )

        deployment = self.deployment

        secrets = self.secrets.to_dict()

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "deployment": deployment,
                "secrets": secrets,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.deployment_secrets_response_200_secrets import (
            DeploymentSecretsResponse200Secrets,
        )

        d = dict(src_dict)
        deployment = d.pop("deployment")

        secrets = DeploymentSecretsResponse200Secrets.from_dict(d.pop("secrets"))

        deployment_secrets_response_200 = cls(
            deployment=deployment,
            secrets=secrets,
        )

        deployment_secrets_response_200.additional_properties = d
        return deployment_secrets_response_200

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
