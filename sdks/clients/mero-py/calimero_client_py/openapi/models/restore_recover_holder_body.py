from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from ..types import UNSET, Unset
from typing import cast
from typing import Union

if TYPE_CHECKING:
    from ..models.restore_recover_holder_body_apps_item import (
        RestoreRecoverHolderBodyAppsItem,
    )
    from ..models.restore_recover_holder_body_code_item import (
        RestoreRecoverHolderBodyCodeItem,
    )
    from ..models.restore_recover_holder_body_slice import RestoreRecoverHolderBodySlice
    from ..models.operator_proof import OperatorProof


T = TypeVar("T", bound="RestoreRecoverHolderBody")


@_attrs_define
class RestoreRecoverHolderBody:
    """
    Attributes:
        holder (str):
        slice_ (RestoreRecoverHolderBodySlice):
        apps (Union[Unset, list['RestoreRecoverHolderBodyAppsItem']]):
        code (Union[Unset, list['RestoreRecoverHolderBodyCodeItem']]):
        operator_proof (Union[Unset, OperatorProof]): The operator key's Ed25519 signature (hex) over the statement a
            `428 OPERATOR_SIGNATURE_REQUIRED` named, with that statement's nonce.
    """

    holder: str
    slice_: "RestoreRecoverHolderBodySlice"
    apps: Union[Unset, list["RestoreRecoverHolderBodyAppsItem"]] = UNSET
    code: Union[Unset, list["RestoreRecoverHolderBodyCodeItem"]] = UNSET
    operator_proof: Union[Unset, "OperatorProof"] = UNSET

    def to_dict(self) -> dict[str, Any]:
        from ..models.restore_recover_holder_body_apps_item import (
            RestoreRecoverHolderBodyAppsItem,
        )
        from ..models.restore_recover_holder_body_code_item import (
            RestoreRecoverHolderBodyCodeItem,
        )
        from ..models.restore_recover_holder_body_slice import (
            RestoreRecoverHolderBodySlice,
        )
        from ..models.operator_proof import OperatorProof

        holder = self.holder

        slice_ = self.slice_.to_dict()

        apps: Union[Unset, list[dict[str, Any]]] = UNSET
        if not isinstance(self.apps, Unset):
            apps = []
            for apps_item_data in self.apps:
                apps_item = apps_item_data.to_dict()
                apps.append(apps_item)

        code: Union[Unset, list[dict[str, Any]]] = UNSET
        if not isinstance(self.code, Unset):
            code = []
            for code_item_data in self.code:
                code_item = code_item_data.to_dict()
                code.append(code_item)

        operator_proof: Union[Unset, dict[str, Any]] = UNSET
        if not isinstance(self.operator_proof, Unset):
            operator_proof = self.operator_proof.to_dict()

        field_dict: dict[str, Any] = {}

        field_dict.update(
            {
                "holder": holder,
                "slice": slice_,
            }
        )
        if apps is not UNSET:
            field_dict["apps"] = apps
        if code is not UNSET:
            field_dict["code"] = code
        if operator_proof is not UNSET:
            field_dict["operatorProof"] = operator_proof

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.restore_recover_holder_body_apps_item import (
            RestoreRecoverHolderBodyAppsItem,
        )
        from ..models.restore_recover_holder_body_code_item import (
            RestoreRecoverHolderBodyCodeItem,
        )
        from ..models.restore_recover_holder_body_slice import (
            RestoreRecoverHolderBodySlice,
        )
        from ..models.operator_proof import OperatorProof

        d = dict(src_dict)
        holder = d.pop("holder")

        slice_ = RestoreRecoverHolderBodySlice.from_dict(d.pop("slice"))

        apps = []
        _apps = d.pop("apps", UNSET)
        for apps_item_data in _apps or []:
            apps_item = RestoreRecoverHolderBodyAppsItem.from_dict(apps_item_data)

            apps.append(apps_item)

        code = []
        _code = d.pop("code", UNSET)
        for code_item_data in _code or []:
            code_item = RestoreRecoverHolderBodyCodeItem.from_dict(code_item_data)

            code.append(code_item)

        _operator_proof = d.pop("operatorProof", UNSET)
        operator_proof: Union[Unset, OperatorProof]
        if isinstance(_operator_proof, Unset):
            operator_proof = UNSET
        else:
            operator_proof = OperatorProof.from_dict(_operator_proof)

        restore_recover_holder_body = cls(
            holder=holder,
            slice_=slice_,
            apps=apps,
            code=code,
            operator_proof=operator_proof,
        )

        return restore_recover_holder_body
