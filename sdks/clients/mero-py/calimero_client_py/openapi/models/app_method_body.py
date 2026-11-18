from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from ..models.app_method_body_consistency import AppMethodBodyConsistency
from ..types import UNSET, Unset
from typing import cast
from typing import Union

if TYPE_CHECKING:
    from ..models.confirm import Confirm


T = TypeVar("T", bound="AppMethodBody")


@_attrs_define
class AppMethodBody:
    """
    Attributes:
        args (Union[Unset, Any]): Free-form JSON: its shape is not this API's (see the description around it for whose
            it is).
        idempotency_key (Union[Unset, str]):
        confirm (Union[Unset, Confirm]): A passkey assertion over the challenge a `412 CONFIRM_REQUIRED` named: the same
            request sent again with it approves exactly that request.
        consistency (Union[Unset, AppMethodBodyConsistency]):
        min_index (Union[Unset, int]):
    """

    args: Union[Unset, Any] = UNSET
    idempotency_key: Union[Unset, str] = UNSET
    confirm: Union[Unset, "Confirm"] = UNSET
    consistency: Union[Unset, AppMethodBodyConsistency] = UNSET
    min_index: Union[Unset, int] = UNSET

    def to_dict(self) -> dict[str, Any]:
        from ..models.confirm import Confirm

        args = self.args

        idempotency_key = self.idempotency_key

        confirm: Union[Unset, dict[str, Any]] = UNSET
        if not isinstance(self.confirm, Unset):
            confirm = self.confirm.to_dict()

        consistency: Union[Unset, str] = UNSET
        if not isinstance(self.consistency, Unset):
            consistency = self.consistency.value

        min_index = self.min_index

        field_dict: dict[str, Any] = {}

        field_dict.update({})
        if args is not UNSET:
            field_dict["args"] = args
        if idempotency_key is not UNSET:
            field_dict["idempotencyKey"] = idempotency_key
        if confirm is not UNSET:
            field_dict["confirm"] = confirm
        if consistency is not UNSET:
            field_dict["consistency"] = consistency
        if min_index is not UNSET:
            field_dict["minIndex"] = min_index

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.confirm import Confirm

        d = dict(src_dict)
        args = d.pop("args", UNSET)

        idempotency_key = d.pop("idempotencyKey", UNSET)

        _confirm = d.pop("confirm", UNSET)
        confirm: Union[Unset, Confirm]
        if isinstance(_confirm, Unset):
            confirm = UNSET
        else:
            confirm = Confirm.from_dict(_confirm)

        _consistency = d.pop("consistency", UNSET)
        consistency: Union[Unset, AppMethodBodyConsistency]
        if isinstance(_consistency, Unset):
            consistency = UNSET
        else:
            consistency = AppMethodBodyConsistency(_consistency)

        min_index = d.pop("minIndex", UNSET)

        app_method_body = cls(
            args=args,
            idempotency_key=idempotency_key,
            confirm=confirm,
            consistency=consistency,
            min_index=min_index,
        )

        return app_method_body
