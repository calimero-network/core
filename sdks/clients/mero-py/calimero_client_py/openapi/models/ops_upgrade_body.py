from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from ..types import UNSET, Unset
from typing import cast
from typing import Union

if TYPE_CHECKING:
    from ..models.operator_proof import OperatorProof


T = TypeVar("T", bound="OpsUpgradeBody")


@_attrs_define
class OpsUpgradeBody:
    """
    Attributes:
        bundle (Union[Unset, str]):
        package (Union[Unset, str]):
        version (Union[Unset, str]):
        service (Union[Unset, str]):
        migrate (Union[Unset, str]):
        operator_proof (Union[Unset, OperatorProof]): The operator key's Ed25519 signature (hex) over the statement a
            `428 OPERATOR_SIGNATURE_REQUIRED` named, with that statement's nonce.
    """

    bundle: Union[Unset, str] = UNSET
    package: Union[Unset, str] = UNSET
    version: Union[Unset, str] = UNSET
    service: Union[Unset, str] = UNSET
    migrate: Union[Unset, str] = UNSET
    operator_proof: Union[Unset, "OperatorProof"] = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.operator_proof import OperatorProof

        bundle = self.bundle

        package = self.package

        version = self.version

        service = self.service

        migrate = self.migrate

        operator_proof: Union[Unset, dict[str, Any]] = UNSET
        if not isinstance(self.operator_proof, Unset):
            operator_proof = self.operator_proof.to_dict()

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update({})
        if bundle is not UNSET:
            field_dict["bundle"] = bundle
        if package is not UNSET:
            field_dict["package"] = package
        if version is not UNSET:
            field_dict["version"] = version
        if service is not UNSET:
            field_dict["service"] = service
        if migrate is not UNSET:
            field_dict["migrate"] = migrate
        if operator_proof is not UNSET:
            field_dict["operatorProof"] = operator_proof

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.operator_proof import OperatorProof

        d = dict(src_dict)
        bundle = d.pop("bundle", UNSET)

        package = d.pop("package", UNSET)

        version = d.pop("version", UNSET)

        service = d.pop("service", UNSET)

        migrate = d.pop("migrate", UNSET)

        _operator_proof = d.pop("operatorProof", UNSET)
        operator_proof: Union[Unset, OperatorProof]
        if isinstance(_operator_proof, Unset):
            operator_proof = UNSET
        else:
            operator_proof = OperatorProof.from_dict(_operator_proof)

        ops_upgrade_body = cls(
            bundle=bundle,
            package=package,
            version=version,
            service=service,
            migrate=migrate,
            operator_proof=operator_proof,
        )

        ops_upgrade_body.additional_properties = d
        return ops_upgrade_body

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
