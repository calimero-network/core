from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

T = TypeVar("T", bound="DeploymentSecretSet")


@_attrs_define
class DeploymentSecretSet:
    """What `SetDeploymentSecret` answers: never the value, its fingerprint.

    Attributes:
        deployment (str):
        name (str):
        fingerprint (str):
        set_by (str):
        set_at (int):
    """

    deployment: str
    name: str
    fingerprint: str
    set_by: str
    set_at: int
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        deployment = self.deployment

        name = self.name

        fingerprint = self.fingerprint

        set_by = self.set_by

        set_at = self.set_at

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "deployment": deployment,
                "name": name,
                "fingerprint": fingerprint,
                "setBy": set_by,
                "setAt": set_at,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        deployment = d.pop("deployment")

        name = d.pop("name")

        fingerprint = d.pop("fingerprint")

        set_by = d.pop("setBy")

        set_at = d.pop("setAt")

        deployment_secret_set = cls(
            deployment=deployment,
            name=name,
            fingerprint=fingerprint,
            set_by=set_by,
            set_at=set_at,
        )

        deployment_secret_set.additional_properties = d
        return deployment_secret_set

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
