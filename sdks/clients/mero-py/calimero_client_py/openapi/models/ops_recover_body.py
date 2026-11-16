from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from ..types import UNSET, Unset
from typing import cast
from typing import Union

if TYPE_CHECKING:
    from ..models.ops_recover_body_code_item import OpsRecoverBodyCodeItem
    from ..models.ops_recover_body_system import OpsRecoverBodySystem


T = TypeVar("T", bound="OpsRecoverBody")


@_attrs_define
class OpsRecoverBody:
    """
    Attributes:
        system (OpsRecoverBodySystem):
        code (Union[Unset, list['OpsRecoverBodyCodeItem']]):
    """

    system: "OpsRecoverBodySystem"
    code: Union[Unset, list["OpsRecoverBodyCodeItem"]] = UNSET

    def to_dict(self) -> dict[str, Any]:
        from ..models.ops_recover_body_code_item import OpsRecoverBodyCodeItem
        from ..models.ops_recover_body_system import OpsRecoverBodySystem

        system = self.system.to_dict()

        code: Union[Unset, list[dict[str, Any]]] = UNSET
        if not isinstance(self.code, Unset):
            code = []
            for code_item_data in self.code:
                code_item = code_item_data.to_dict()
                code.append(code_item)

        field_dict: dict[str, Any] = {}

        field_dict.update(
            {
                "system": system,
            }
        )
        if code is not UNSET:
            field_dict["code"] = code

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.ops_recover_body_code_item import OpsRecoverBodyCodeItem
        from ..models.ops_recover_body_system import OpsRecoverBodySystem

        d = dict(src_dict)
        system = OpsRecoverBodySystem.from_dict(d.pop("system"))

        code = []
        _code = d.pop("code", UNSET)
        for code_item_data in _code or []:
            code_item = OpsRecoverBodyCodeItem.from_dict(code_item_data)

            code.append(code_item)

        ops_recover_body = cls(
            system=system,
            code=code,
        )

        return ops_recover_body
