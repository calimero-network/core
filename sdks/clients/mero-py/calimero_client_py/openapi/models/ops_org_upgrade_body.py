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
    from ..models.confirm import Confirm


T = TypeVar("T", bound="OpsOrgUpgradeBody")


@_attrs_define
class OpsOrgUpgradeBody:
    """
    Attributes:
        package (str):
        version (str):
        team (Union[Unset, str]):
        cascade (Union[Unset, bool]):
        migrate (Union[Unset, str]):
        confirm (Union[Unset, Confirm]): A passkey assertion over the challenge a `412 CONFIRM_REQUIRED` named: the same
            request sent again with it approves exactly that request.
        operator_proof (Union[Unset, OperatorProof]): The operator key's Ed25519 signature (hex) over the statement a
            `428 OPERATOR_SIGNATURE_REQUIRED` named, with that statement's nonce.
    """

    package: str
    version: str
    team: Union[Unset, str] = UNSET
    cascade: Union[Unset, bool] = UNSET
    migrate: Union[Unset, str] = UNSET
    confirm: Union[Unset, "Confirm"] = UNSET
    operator_proof: Union[Unset, "OperatorProof"] = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.operator_proof import OperatorProof
        from ..models.confirm import Confirm

        package = self.package

        version = self.version

        team = self.team

        cascade = self.cascade

        migrate = self.migrate

        confirm: Union[Unset, dict[str, Any]] = UNSET
        if not isinstance(self.confirm, Unset):
            confirm = self.confirm.to_dict()

        operator_proof: Union[Unset, dict[str, Any]] = UNSET
        if not isinstance(self.operator_proof, Unset):
            operator_proof = self.operator_proof.to_dict()

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "package": package,
                "version": version,
            }
        )
        if team is not UNSET:
            field_dict["team"] = team
        if cascade is not UNSET:
            field_dict["cascade"] = cascade
        if migrate is not UNSET:
            field_dict["migrate"] = migrate
        if confirm is not UNSET:
            field_dict["confirm"] = confirm
        if operator_proof is not UNSET:
            field_dict["operatorProof"] = operator_proof

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.operator_proof import OperatorProof
        from ..models.confirm import Confirm

        d = dict(src_dict)
        package = d.pop("package")

        version = d.pop("version")

        team = d.pop("team", UNSET)

        cascade = d.pop("cascade", UNSET)

        migrate = d.pop("migrate", UNSET)

        _confirm = d.pop("confirm", UNSET)
        confirm: Union[Unset, Confirm]
        if isinstance(_confirm, Unset):
            confirm = UNSET
        else:
            confirm = Confirm.from_dict(_confirm)

        _operator_proof = d.pop("operatorProof", UNSET)
        operator_proof: Union[Unset, OperatorProof]
        if isinstance(_operator_proof, Unset):
            operator_proof = UNSET
        else:
            operator_proof = OperatorProof.from_dict(_operator_proof)

        ops_org_upgrade_body = cls(
            package=package,
            version=version,
            team=team,
            cascade=cascade,
            migrate=migrate,
            confirm=confirm,
            operator_proof=operator_proof,
        )

        ops_org_upgrade_body.additional_properties = d
        return ops_org_upgrade_body

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
