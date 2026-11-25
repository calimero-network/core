from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from ..models.account_view_via import AccountViewVia
from typing import cast
from typing import cast, Union

if TYPE_CHECKING:
    from ..models.account_passkey import AccountPasskey
    from ..models.account_org import AccountOrg
    from ..models.account_recovery_pending import AccountRecoveryPending


T = TypeVar("T", bound="AccountView")


@_attrs_define
class AccountView:
    """`GET /auth/me`: the caller's account, its passkeys and organizations, how the caller came and its recovery state.

    Attributes:
        account_id (str):
        name (str):
        passkeys (list['AccountPasskey']):
        orgs (list['AccountOrg']):
        via (AccountViewVia):
        passkey_id (Union[None, str]):
        app (Union[None, str]):
        origin (Union[None, str]):
        recovery_code (bool):
        recovery_pending (Union['AccountRecoveryPending', None]):
    """

    account_id: str
    name: str
    passkeys: list["AccountPasskey"]
    orgs: list["AccountOrg"]
    via: AccountViewVia
    passkey_id: Union[None, str]
    app: Union[None, str]
    origin: Union[None, str]
    recovery_code: bool
    recovery_pending: Union["AccountRecoveryPending", None]
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.account_passkey import AccountPasskey
        from ..models.account_org import AccountOrg
        from ..models.account_recovery_pending import AccountRecoveryPending

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

        app: Union[None, str]
        app = self.app

        origin: Union[None, str]
        origin = self.origin

        recovery_code = self.recovery_code

        recovery_pending: Union[None, dict[str, Any]]
        if isinstance(self.recovery_pending, AccountRecoveryPending):
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
                "app": app,
                "origin": origin,
                "recoveryCode": recovery_code,
                "recoveryPending": recovery_pending,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.account_passkey import AccountPasskey
        from ..models.account_org import AccountOrg
        from ..models.account_recovery_pending import AccountRecoveryPending

        d = dict(src_dict)
        account_id = d.pop("accountId")

        name = d.pop("name")

        passkeys = []
        _passkeys = d.pop("passkeys")
        for passkeys_item_data in _passkeys:
            passkeys_item = AccountPasskey.from_dict(passkeys_item_data)

            passkeys.append(passkeys_item)

        orgs = []
        _orgs = d.pop("orgs")
        for orgs_item_data in _orgs:
            orgs_item = AccountOrg.from_dict(orgs_item_data)

            orgs.append(orgs_item)

        via = AccountViewVia(d.pop("via"))

        def _parse_passkey_id(data: object) -> Union[None, str]:
            if data is None:
                return data
            return cast(Union[None, str], data)

        passkey_id = _parse_passkey_id(d.pop("passkeyId"))

        def _parse_app(data: object) -> Union[None, str]:
            if data is None:
                return data
            return cast(Union[None, str], data)

        app = _parse_app(d.pop("app"))

        def _parse_origin(data: object) -> Union[None, str]:
            if data is None:
                return data
            return cast(Union[None, str], data)

        origin = _parse_origin(d.pop("origin"))

        recovery_code = d.pop("recoveryCode")

        def _parse_recovery_pending(
            data: object,
        ) -> Union["AccountRecoveryPending", None]:
            if data is None:
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                recovery_pending_type_0 = AccountRecoveryPending.from_dict(data)

                return recovery_pending_type_0
            except:  # noqa: E722
                pass
            return cast(Union["AccountRecoveryPending", None], data)

        recovery_pending = _parse_recovery_pending(d.pop("recoveryPending"))

        account_view = cls(
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

        account_view.additional_properties = d
        return account_view

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
