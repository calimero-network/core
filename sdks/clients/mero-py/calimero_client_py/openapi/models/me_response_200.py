from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from ..models.me_response_200_via import MeResponse200Via
from ..types import UNSET, Unset
from typing import cast
from typing import cast, Union
from typing import Union

if TYPE_CHECKING:
    from ..models.me_response_200_orgs_item import MeResponse200OrgsItem
    from ..models.me_response_200_passkeys_item import MeResponse200PasskeysItem
    from ..models.me_response_200_recovery_pending_type_0 import (
        MeResponse200RecoveryPendingType0,
    )


T = TypeVar("T", bound="MeResponse200")


@_attrs_define
class MeResponse200:
    """
    Attributes:
        account_id (str):
        name (str):
        passkeys (list['MeResponse200PasskeysItem']):
        orgs (list['MeResponse200OrgsItem']):
        via (MeResponse200Via):
        passkey_id (Union[None, str]):
        app (Union[None, Unset, str]):
        origin (Union[None, Unset, str]):
        recovery_code (Union[Unset, bool]):
        recovery_pending (Union['MeResponse200RecoveryPendingType0', None, Unset]):
    """

    account_id: str
    name: str
    passkeys: list["MeResponse200PasskeysItem"]
    orgs: list["MeResponse200OrgsItem"]
    via: MeResponse200Via
    passkey_id: Union[None, str]
    app: Union[None, Unset, str] = UNSET
    origin: Union[None, Unset, str] = UNSET
    recovery_code: Union[Unset, bool] = UNSET
    recovery_pending: Union["MeResponse200RecoveryPendingType0", None, Unset] = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.me_response_200_orgs_item import MeResponse200OrgsItem
        from ..models.me_response_200_passkeys_item import MeResponse200PasskeysItem
        from ..models.me_response_200_recovery_pending_type_0 import (
            MeResponse200RecoveryPendingType0,
        )

        account_id = self.account_id

        name = self.name

        passkeys = []
        for passkeys_item_data in self.passkeys:
            passkeys_item = passkeys_item_data.to_dict()
            passkeys.append(passkeys_item)

        orgs = []
        for orgs_item_data in self.orgs:
            orgs_item = orgs_item_data.to_dict()
            orgs.append(orgs_item)

        via = self.via.value

        passkey_id: Union[None, str]
        passkey_id = self.passkey_id

        app: Union[None, Unset, str]
        if isinstance(self.app, Unset):
            app = UNSET
        else:
            app = self.app

        origin: Union[None, Unset, str]
        if isinstance(self.origin, Unset):
            origin = UNSET
        else:
            origin = self.origin

        recovery_code = self.recovery_code

        recovery_pending: Union[None, Unset, dict[str, Any]]
        if isinstance(self.recovery_pending, Unset):
            recovery_pending = UNSET
        elif isinstance(self.recovery_pending, MeResponse200RecoveryPendingType0):
            recovery_pending = self.recovery_pending.to_dict()
        else:
            recovery_pending = self.recovery_pending

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "accountId": account_id,
                "name": name,
                "passkeys": passkeys,
                "orgs": orgs,
                "via": via,
                "passkeyId": passkey_id,
            }
        )
        if app is not UNSET:
            field_dict["app"] = app
        if origin is not UNSET:
            field_dict["origin"] = origin
        if recovery_code is not UNSET:
            field_dict["recoveryCode"] = recovery_code
        if recovery_pending is not UNSET:
            field_dict["recoveryPending"] = recovery_pending

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.me_response_200_orgs_item import MeResponse200OrgsItem
        from ..models.me_response_200_passkeys_item import MeResponse200PasskeysItem
        from ..models.me_response_200_recovery_pending_type_0 import (
            MeResponse200RecoveryPendingType0,
        )

        d = dict(src_dict)
        account_id = d.pop("accountId")

        name = d.pop("name")

        passkeys = []
        _passkeys = d.pop("passkeys")
        for passkeys_item_data in _passkeys:
            passkeys_item = MeResponse200PasskeysItem.from_dict(passkeys_item_data)

            passkeys.append(passkeys_item)

        orgs = []
        _orgs = d.pop("orgs")
        for orgs_item_data in _orgs:
            orgs_item = MeResponse200OrgsItem.from_dict(orgs_item_data)

            orgs.append(orgs_item)

        via = MeResponse200Via(d.pop("via"))

        def _parse_passkey_id(data: object) -> Union[None, str]:
            if data is None:
                return data
            return cast(Union[None, str], data)

        passkey_id = _parse_passkey_id(d.pop("passkeyId"))

        def _parse_app(data: object) -> Union[None, Unset, str]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, str], data)

        app = _parse_app(d.pop("app", UNSET))

        def _parse_origin(data: object) -> Union[None, Unset, str]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, str], data)

        origin = _parse_origin(d.pop("origin", UNSET))

        recovery_code = d.pop("recoveryCode", UNSET)

        def _parse_recovery_pending(
            data: object,
        ) -> Union["MeResponse200RecoveryPendingType0", None, Unset]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                recovery_pending_type_0 = MeResponse200RecoveryPendingType0.from_dict(
                    data
                )

                return recovery_pending_type_0
            except:  # noqa: E722
                pass
            return cast(Union["MeResponse200RecoveryPendingType0", None, Unset], data)

        recovery_pending = _parse_recovery_pending(d.pop("recoveryPending", UNSET))

        me_response_200 = cls(
            account_id=account_id,
            name=name,
            passkeys=passkeys,
            orgs=orgs,
            via=via,
            passkey_id=passkey_id,
            app=app,
            origin=origin,
            recovery_code=recovery_code,
            recovery_pending=recovery_pending,
        )

        me_response_200.additional_properties = d
        return me_response_200

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
