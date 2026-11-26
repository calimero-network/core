from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from ..models.report_upgrade_actor_kind import ReportUpgradeActorKind
from ..models.report_upgrade_class import ReportUpgradeClass
from ..types import UNSET, Unset
from typing import cast
from typing import cast, Union
from typing import Union

if TYPE_CHECKING:
    from ..models.update_reason_type_8 import UpdateReasonType8
    from ..models.update_reason_type_4 import UpdateReasonType4
    from ..models.update_reason_type_3 import UpdateReasonType3
    from ..models.bundle_release import BundleRelease
    from ..models.update_reason_type_7 import UpdateReasonType7
    from ..models.update_reason_type_1 import UpdateReasonType1
    from ..models.update_reason_type_6 import UpdateReasonType6
    from ..models.update_reason_type_0 import UpdateReasonType0
    from ..models.update_reason_type_5 import UpdateReasonType5
    from ..models.update_reason_type_2 import UpdateReasonType2


T = TypeVar("T", bound="ReportUpgrade")


@_attrs_define
class ReportUpgrade:
    """An app's last upgrade as a report shows it. `actor`, `error`, `run` and `apiKey` are shown to the app's people,
    never to the operator.

        Attributes:
            from_ (BundleRelease): A bundle by its digest, with the release it came from where the reader may see it.
            to (BundleRelease): A bundle by its digest, with the release it came from where the reader may see it.
            at (int):
            ok (bool):
            actor_kind (ReportUpgradeActorKind):
            class_ (Union[Unset, ReportUpgradeClass]):
            actor (Union[Unset, str]):
            error (Union[Unset, Any]): Free-form JSON: its shape is not this API's (see the description around it for whose
                it is).
            run (Union[Unset, str]):
            api_key (Union[Unset, str]):
            reasons (Union[Unset, list[Union['UpdateReasonType0', 'UpdateReasonType1', 'UpdateReasonType2',
                'UpdateReasonType3', 'UpdateReasonType4', 'UpdateReasonType5', 'UpdateReasonType6', 'UpdateReasonType7',
                'UpdateReasonType8']]]):
            approved_by (Union[Unset, str]):
    """

    from_: "BundleRelease"
    to: "BundleRelease"
    at: int
    ok: bool
    actor_kind: ReportUpgradeActorKind
    class_: Union[Unset, ReportUpgradeClass] = UNSET
    actor: Union[Unset, str] = UNSET
    error: Union[Unset, Any] = UNSET
    run: Union[Unset, str] = UNSET
    api_key: Union[Unset, str] = UNSET
    reasons: Union[
        Unset,
        list[
            Union[
                "UpdateReasonType0",
                "UpdateReasonType1",
                "UpdateReasonType2",
                "UpdateReasonType3",
                "UpdateReasonType4",
                "UpdateReasonType5",
                "UpdateReasonType6",
                "UpdateReasonType7",
                "UpdateReasonType8",
            ]
        ],
    ] = UNSET
    approved_by: Union[Unset, str] = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.update_reason_type_8 import UpdateReasonType8
        from ..models.update_reason_type_4 import UpdateReasonType4
        from ..models.update_reason_type_3 import UpdateReasonType3
        from ..models.bundle_release import BundleRelease
        from ..models.update_reason_type_7 import UpdateReasonType7
        from ..models.update_reason_type_1 import UpdateReasonType1
        from ..models.update_reason_type_6 import UpdateReasonType6
        from ..models.update_reason_type_0 import UpdateReasonType0
        from ..models.update_reason_type_5 import UpdateReasonType5
        from ..models.update_reason_type_2 import UpdateReasonType2

        from_ = self.from_.to_dict()

        to = self.to.to_dict()

        at = self.at

        ok = self.ok

        actor_kind = self.actor_kind.value

        class_: Union[Unset, str] = UNSET
        if not isinstance(self.class_, Unset):
            class_ = self.class_.value

        actor = self.actor

        error = self.error

        run = self.run

        api_key = self.api_key

        reasons: Union[Unset, list[dict[str, Any]]] = UNSET
        if not isinstance(self.reasons, Unset):
            reasons = []
            for reasons_item_data in self.reasons:
                reasons_item: dict[str, Any]
                if isinstance(reasons_item_data, UpdateReasonType0):
                    reasons_item = reasons_item_data.to_dict()
                elif isinstance(reasons_item_data, UpdateReasonType1):
                    reasons_item = reasons_item_data.to_dict()
                elif isinstance(reasons_item_data, UpdateReasonType2):
                    reasons_item = reasons_item_data.to_dict()
                elif isinstance(reasons_item_data, UpdateReasonType3):
                    reasons_item = reasons_item_data.to_dict()
                elif isinstance(reasons_item_data, UpdateReasonType4):
                    reasons_item = reasons_item_data.to_dict()
                elif isinstance(reasons_item_data, UpdateReasonType5):
                    reasons_item = reasons_item_data.to_dict()
                elif isinstance(reasons_item_data, UpdateReasonType6):
                    reasons_item = reasons_item_data.to_dict()
                elif isinstance(reasons_item_data, UpdateReasonType7):
                    reasons_item = reasons_item_data.to_dict()
                else:
                    reasons_item = reasons_item_data.to_dict()

                reasons.append(reasons_item)

        approved_by = self.approved_by

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "from": from_,
                "to": to,
                "at": at,
                "ok": ok,
                "actorKind": actor_kind,
            }
        )
        if class_ is not UNSET:
            field_dict["class"] = class_
        if actor is not UNSET:
            field_dict["actor"] = actor
        if error is not UNSET:
            field_dict["error"] = error
        if run is not UNSET:
            field_dict["run"] = run
        if api_key is not UNSET:
            field_dict["apiKey"] = api_key
        if reasons is not UNSET:
            field_dict["reasons"] = reasons
        if approved_by is not UNSET:
            field_dict["approvedBy"] = approved_by

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.update_reason_type_8 import UpdateReasonType8
        from ..models.update_reason_type_4 import UpdateReasonType4
        from ..models.update_reason_type_3 import UpdateReasonType3
        from ..models.bundle_release import BundleRelease
        from ..models.update_reason_type_7 import UpdateReasonType7
        from ..models.update_reason_type_1 import UpdateReasonType1
        from ..models.update_reason_type_6 import UpdateReasonType6
        from ..models.update_reason_type_0 import UpdateReasonType0
        from ..models.update_reason_type_5 import UpdateReasonType5
        from ..models.update_reason_type_2 import UpdateReasonType2

        d = dict(src_dict)
        from_ = BundleRelease.from_dict(d.pop("from"))

        to = BundleRelease.from_dict(d.pop("to"))

        at = d.pop("at")

        ok = d.pop("ok")

        actor_kind = ReportUpgradeActorKind(d.pop("actorKind"))

        _class_ = d.pop("class", UNSET)
        class_: Union[Unset, ReportUpgradeClass]
        if isinstance(_class_, Unset):
            class_ = UNSET
        else:
            class_ = ReportUpgradeClass(_class_)

        actor = d.pop("actor", UNSET)

        error = d.pop("error", UNSET)

        run = d.pop("run", UNSET)

        api_key = d.pop("apiKey", UNSET)

        reasons = []
        _reasons = d.pop("reasons", UNSET)
        for reasons_item_data in _reasons or []:

            def _parse_reasons_item(
                data: object,
            ) -> Union[
                "UpdateReasonType0",
                "UpdateReasonType1",
                "UpdateReasonType2",
                "UpdateReasonType3",
                "UpdateReasonType4",
                "UpdateReasonType5",
                "UpdateReasonType6",
                "UpdateReasonType7",
                "UpdateReasonType8",
            ]:
                try:
                    if not isinstance(data, dict):
                        raise TypeError()
                    componentsschemas_update_reason_type_0 = (
                        UpdateReasonType0.from_dict(data)
                    )

                    return componentsschemas_update_reason_type_0
                except:  # noqa: E722
                    pass
                try:
                    if not isinstance(data, dict):
                        raise TypeError()
                    componentsschemas_update_reason_type_1 = (
                        UpdateReasonType1.from_dict(data)
                    )

                    return componentsschemas_update_reason_type_1
                except:  # noqa: E722
                    pass
                try:
                    if not isinstance(data, dict):
                        raise TypeError()
                    componentsschemas_update_reason_type_2 = (
                        UpdateReasonType2.from_dict(data)
                    )

                    return componentsschemas_update_reason_type_2
                except:  # noqa: E722
                    pass
                try:
                    if not isinstance(data, dict):
                        raise TypeError()
                    componentsschemas_update_reason_type_3 = (
                        UpdateReasonType3.from_dict(data)
                    )

                    return componentsschemas_update_reason_type_3
                except:  # noqa: E722
                    pass
                try:
                    if not isinstance(data, dict):
                        raise TypeError()
                    componentsschemas_update_reason_type_4 = (
                        UpdateReasonType4.from_dict(data)
                    )

                    return componentsschemas_update_reason_type_4
                except:  # noqa: E722
                    pass
                try:
                    if not isinstance(data, dict):
                        raise TypeError()
                    componentsschemas_update_reason_type_5 = (
                        UpdateReasonType5.from_dict(data)
                    )

                    return componentsschemas_update_reason_type_5
                except:  # noqa: E722
                    pass
                try:
                    if not isinstance(data, dict):
                        raise TypeError()
                    componentsschemas_update_reason_type_6 = (
                        UpdateReasonType6.from_dict(data)
                    )

                    return componentsschemas_update_reason_type_6
                except:  # noqa: E722
                    pass
                try:
                    if not isinstance(data, dict):
                        raise TypeError()
                    componentsschemas_update_reason_type_7 = (
                        UpdateReasonType7.from_dict(data)
                    )

                    return componentsschemas_update_reason_type_7
                except:  # noqa: E722
                    pass
                if not isinstance(data, dict):
                    raise TypeError()
                componentsschemas_update_reason_type_8 = UpdateReasonType8.from_dict(
                    data
                )

                return componentsschemas_update_reason_type_8

            reasons_item = _parse_reasons_item(reasons_item_data)

            reasons.append(reasons_item)

        approved_by = d.pop("approvedBy", UNSET)

        report_upgrade = cls(
            from_=from_,
            to=to,
            at=at,
            ok=ok,
            actor_kind=actor_kind,
            class_=class_,
            actor=actor,
            error=error,
            run=run,
            api_key=api_key,
            reasons=reasons,
            approved_by=approved_by,
        )

        report_upgrade.additional_properties = d
        return report_upgrade

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
