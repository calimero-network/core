from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

T = TypeVar("T", bound="ApiKeyUpgrades")


@_attrs_define
class ApiKeyUpgrades:
    """
    Attributes:
        app (str):
        package (str):
        signer (str):
        proof_key (str):
    """

    app: str
    package: str
    signer: str
    proof_key: str
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        app = self.app

        package = self.package

        signer = self.signer

        proof_key = self.proof_key

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "app": app,
                "package": package,
                "signer": signer,
                "proofKey": proof_key,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        app = d.pop("app")

        package = d.pop("package")

        signer = d.pop("signer")

        proof_key = d.pop("proofKey")

        api_key_upgrades = cls(
            app=app,
            package=package,
            signer=signer,
            proof_key=proof_key,
        )

        api_key_upgrades.additional_properties = d
        return api_key_upgrades

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
