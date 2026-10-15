//! Generic, session-bound question answers. Native schemas stay in their adapters.
use anyhow::{Result, bail};
use latte_work_protocol::{QuestionAnswer, UserQuestion};
use std::collections::HashSet;

pub fn validate(questions: &[UserQuestion], answers: &[QuestionAnswer]) -> Result<()> {
    if answers.len() != questions.len() {
        bail!("请回答所有问题");
    }
    let mut seen = HashSet::new();
    for answer in answers {
        let question = questions
            .iter()
            .find(|q| q.id == answer.question_id)
            .ok_or_else(|| anyhow::anyhow!("回答不属于当前问题"))?;
        if !seen.insert(&answer.question_id) {
            bail!("问题回答重复");
        }
        let mut selected = HashSet::new();
        if answer
            .option_indices
            .iter()
            .any(|index| !selected.insert(index) || *index as usize >= question.options.len())
        {
            bail!("无效的问题选项");
        }
        let text = answer.text.as_deref().unwrap_or("").trim();
        if text.len() > 32 * 1024 || text.chars().count() > 8000 {
            bail!("自填回答超出长度限制");
        }
        let count = selected.len() + usize::from(!text.is_empty());
        if count == 0 || (!question.multi_select && count != 1) {
            bail!("请选择选项或填写回答");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use latte_work_protocol::QuestionOption;
    #[test]
    fn answers_reject_duplicate_indices_ids_missing_questions_and_unbounded_text() {
        let questions = vec![UserQuestion {
            id: "q0".into(),
            header: "".into(),
            text: "Choose".into(),
            options: vec![QuestionOption {
                label: "One".into(),
                description: "".into(),
            }],
            multi_select: true,
        }];
        let answer = QuestionAnswer {
            question_id: "q0".into(),
            option_indices: vec![0],
            text: None,
        };
        assert!(validate(&questions, std::slice::from_ref(&answer)).is_ok());
        for value in [
            QuestionAnswer {
                question_id: "wrong".into(),
                ..answer.clone()
            },
            QuestionAnswer {
                option_indices: vec![0, 0],
                ..answer.clone()
            },
            QuestionAnswer {
                option_indices: vec![1],
                ..answer.clone()
            },
            QuestionAnswer {
                option_indices: vec![],
                text: Some(" ".into()),
                ..answer.clone()
            },
            QuestionAnswer {
                option_indices: vec![],
                text: Some("a".repeat(8001)),
                ..answer.clone()
            },
        ] {
            assert!(validate(&questions, &[value]).is_err());
        }
        assert!(validate(&questions, &[answer.clone(), answer]).is_err());
    }
}
